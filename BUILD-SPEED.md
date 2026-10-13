# Build speed: what forest actually costs, and whether Bazel helps

> **Status, 2026-10-13.** This report was written on 2026-09-21 against `a01055d`
> and is kept as measured. Since then: `apps/forest`'s test suite compiles again
> and CI runs it (`1ae3ff0`, `910f079`); the three cheap levers from §7 — a
> `[profile.dev]`, a cargo-chef dependency layer, and dropping the clang + mold
> stanza — land in #277. "This branch" below means `build/bazel-evaluation`
> (#273, commit `3f3ec65`), where the Bazel/`rules_rust` slice is kept as
> evidence; none of its Bazel files are on `main`.

**Recommendation up front: do not adopt Bazel.** A working `rules_rust` slice of
`apps/forage` is on this branch and it was measured properly — it is 55% slower
to build cold than cargo, 5× slower on the edit-compile loop that developers
actually live in, and its one genuine win (a shared cache turning a 47-second
cold build into 1.1 seconds) is a win against a problem this repository does not
have, because CI already gets a Docker layer cache and nobody's laptop rebuilds
from scratch.

What *is* slow is all elsewhere, and all cheap to fix: a `[profile.dev]` that
nobody set (19% of every debug build, 60% of the disk), a Dockerfile with no
dependency layer (half of every CI build, on every commit), and a Rust test
suite that does not compile on `main` because no workflow has ever run it.

---

## How this was measured

| | |
|---|---|
| Machine | AMD Ryzen 9 9950X, 16 cores / 32 threads, 60 GB RAM, NVMe |
| Toolchain | rustc 1.98.1, cargo 1.98.1 (the `mise.toml` pin), Bazel 8.4.2, rules_rust 0.74.0 |
| Commit | `a01055d` (`main` at the time of measuring) |
| Environment | `SQLX_OFFLINE=true`, `CARGO_NET_OFFLINE=true`, `~/.cargo/registry` pre-populated |

"Cold" means `rm -rf target` with the crates registry already downloaded —
i.e. what you get after `cargo clean` or a branch switch that invalidates
everything, **not** what you get on a fresh machine. `cargo fetch` for both
workspaces takes 0.16 s and 0.12 s against a warm registry, so the download is
not part of any number here.

Two caveats worth holding on to:

- **This machine is faster than any laptop in the team.** A 9950X with 32
  threads is close to the best case for a build that parallelises as well as
  this one does (mean 24.3 concurrent rustc processes). The absolute seconds
  below are a floor; the *ratios* are what transfer.
- **Cold builds vary by a few percent run to run**, and one pair varied by
  10%. Where a difference below is under ~2 seconds, treat it as noise; the
  repeated samples are in the variance note at the end.

Every number below is reproducible from the command next to it.

---

## 1. What the build costs today

### apps/forest — 9 own crates, 11 example crates, 1 `ci` crate, 645 lockfile packages

| Step | Command | Wall | Disk |
|---|---|---:|---:|
| Cold debug build | `cargo build --workspace` | **46.3 s** | 9.69 GB |
| No-op rebuild | `cargo build --workspace` | 0.17 s | |
| One line in a leaf crate | `touch crates/forest/src/main.rs && cargo build --workspace` | 2.0 s | |
| One line in a core crate | `touch crates/forest-models/src/lib.rs && cargo build --workspace` | 3.6 s | |
| First `cargo check` after a build | `cargo check --workspace` | **25.4 s** | |
| `cargo check` after a core change | `cargo check --workspace` | 1.5 s | |
| Build the test binaries | `cargo test --workspace --no-run` | 27.8 s | **fails** |
| Cold release, the two Docker binaries | `cargo build --release --bin forest-server --bin forest-event-migration` | **63.2 s** | 1.33 GB |
| Release after a core change | same, after touching `forest-models` | 32.9 s | |
| Relink only | touch the 44-line `forest-server/src/main.rs` | 8.2 s | |

### apps/forage — 4 own crates, 1 `ci` crate, 504 lockfile packages

| Step | Command | Wall | Disk |
|---|---|---:|---:|
| Cold debug build | `cargo build --workspace` | **30.2 s** | 5.92 GB |
| No-op rebuild | `cargo build --workspace` | 0.13 s | |
| One line in a leaf crate | `touch crates/forage-server/src/main.rs && cargo build --workspace` | 1.9 s | |
| One line in a core crate | `touch crates/forage-core/src/lib.rs && cargo build --workspace` | 2.4 s | |
| First `cargo check` after a build | `cargo check --workspace` | 15.6 s | |
| Build the test binaries | `cargo test --workspace --no-run` | 12.7 s | |
| Run the tests (607 of them) | `cargo test --workspace` | 8.1 s | |
| Run them again, nothing changed | `cargo test --workspace` | 8.1 s | |
| Cold release | `cargo build --release -p forage-server` | **41.2 s** | 1.19 GB |
| Release after a core change | same, after touching `forage-core` | 21.6 s | |
| Frontend install | `npm ci` in `apps/forage/frontend` | 0.6 s | 65 MB |
| Frontend bundle | `npm run build` | 0.8 s | |
| Frontend tests (190 of them) | `npm test` | 0.4 s | |

**Both apps from nothing: 76.5 s of debug build and 15.6 GB of `target/`.**

(Three samples of forest's cold release build gave 62.9 / 63.2 / 63.2 s. A fourth,
run under `--timings` to produce §2, took 69.8 s — the instrumentation costs
about 10% on that build and nothing measurable on the others. §2's *proportions*
are from that instrumented run; the 63.2 s above is the honest wall clock.)

### Three things these tables say that are worth saying out loud

**The edit-compile loop is already fast.** Two to four seconds after a one-line
change, in either app, in either the leaf or the core of the graph. Whatever
"the build is painful" means, it does not mean this.

**`apps/forest`'s test suite does not compile.**

```
error[E0063]: missing field `tags` in initializer of `ProjectMetadata`
  --> crates/forest/examples/set_project_metadata.rs:29:20
error[E0063]: missing fields `gate_deadline` and `gate_waiting_on` in initializer of `PipelineStageState`
error: `SQLX_OFFLINE=true` but there is no cached data for this query   (×4, forest-server acceptance tests)
```

Two stale `examples/`, one stale unit test, and four queries missing from
`.sqlx/`. It has been broken long enough for four separate things to rot,
because `cargo test` appears in no CI workflow — see §3. `apps/forage`'s suite
is fine: 607 tests, 8.1 s.

**The frontend is not a build cost.** 0.4–0.8 s for anything you can ask of it,
and the bundle is committed to `apps/forage/static/` anyway. Neither is protobuf:
`buf generate` output is committed under `crates/*/src/grpc/`, and there is not a
single `build.rs` in either workspace. The "multi-language monorepo" that would
be the strongest argument for Bazel currently costs **zero seconds** of any build.

---

## 2. Where the time actually goes

From `cargo build --workspace --timings` on a cold `apps/forest`:

| | Debug | Release (Docker binaries) |
|---|---:|---:|
| Wall clock (instrumented) | 46.2 s | 69.8 s |
| Summed rustc CPU-seconds | 332.8 s | 383.0 s |
| Effective parallelism | 7.2× (mean 24.3 concurrent units) | 5.5× |
| Third-party crates | 282.7 CPU-s (84.9%) | 335.8 CPU-s (87.7%) |
| First-party crates | 50.1 CPU-s (15.1%) | 47.2 CPU-s (12.3%) |
| Build scripts (C/C++) | 17.3 CPU-s (5.2%) | 40.9 CPU-s (10.7%) |

Which looks like "it's all dependencies" — and in CPU-seconds it is. But the
wall clock tells a different story, because the dependencies all compile in
parallel and the first-party crates do not:

```
release build, 69.8 s as instrumented (63.2 s uninstrumented)
  0 s ─ 25 s   ~550 dependency crates, 32 cores saturated
                 aws-lc-sys build script 17.6 s ─┐ C compiles, on the
                 zstd-sys build script   11.4 s ─┤ critical path early
                 ring build scripts      10.0 s ─┘
 25 s ─ 35 s   forest-grpc-interface (7.8 s), sqlx-postgres, async-nats, tonic
 34.6 s ─ 61.1 s   forest-server: ONE rustc process, 26.5 s
 61.1 s ─ 69.8 s   forest-server: link, 8.7 s
```

**The last 35 seconds of that 70-second release build is a single crate** — half
of it. In the debug build it is the last 26 of 46 seconds. `forest-server` is 58,512 lines in
one compilation unit; `forest` (the CLI) is another 43,842. Nothing that caches
build outputs — not sccache, not Bazel, not a remote cache — makes that number
smaller, because it is not a cache miss, it is one process doing one crate's
work. Only splitting the crate does.

Two smaller things the timings show:

- **`aws-lc-sys` is compiled even though the workspace comment says it should
  not be.** `apps/forest/Cargo.toml` pins `rustls` to the pure-Rust `ring`
  provider precisely to avoid the C library — but `tonic` is configured with
  `tls-aws-lc`, and `reqwest`, `sqlx-core`, `hyper-rustls`, `attohttpc` and
  `rustls-platform-verifier` all turn rustls's `aws-lc-rs` feature on anyway.
  Both providers get built. That is 17.6 CPU-seconds of C for something the
  manifest believes it opted out of.
- **Duplicate major versions.** `apps/forest` compiles 645 units for 558 distinct
  crate names; `apps/forage` 504 for 457. `hyper` 0.14 and 1.9, `reqwest` 0.11,
  0.12 and 0.13, `rustls` 0.21 and 0.23, `http` 0.2 and 1.4, `syn` 1, 2 and 3.
  Most of the old halves arrive through one dependency: `dagger-sdk`, in the
  `ci` crate.

---

## 3. What CI actually does

`.github/workflows/ci.yaml` on a push, from the six most recent runs on `main`:

| Job | Runner | Duration |
|---|---|---:|
| `frontend tests` (npm + vitest + playwright) | `ubuntu-latest` | 45–56 s |
| `build and push forest` — **when `apps/forest` changed** | `32xlarge` | 3 m 20 s (build step 3 m 01 s) |
| `build and push forest` — when it did not | `32xlarge` | 32–46 s |
| `build and push forage` — **when `apps/forage` changed** | `32xlarge` | 3 m 18 s – 3 m 31 s |
| `build and push forage` — when it did not | `32xlarge` | 41 s |
| `deploy {dev,prod}/{forest,forage}` × 4, in parallel | `ubuntu-latest` | ~2 m 40 s each |
| **Total push-to-main wall clock** | | **≈ 7 min** |

Two observations.

**CI runs no Rust tests.** Not `cargo test`, not `cargo clippy`, not `cargo
fmt --check`. The only thing gating a merge is "the Docker image built" plus the
Svelte tests. That is why §1's broken test suite went unnoticed. The frontend
tests were added deliberately with a comment saying "these ran nowhere before;
now they gate the build" — the Rust ones still run nowhere.

**Every commit rebuilds every dependency.** Both Dockerfiles are:

```dockerfile
COPY . .
RUN cargo build --release ...
```

`COPY . .` is keyed on the content of the whole build context, so any source
change invalidates it and the ~500 dependency crates compile again. The
`cache-from: type=gha` in the workflow restores the layer *above* it and nothing
below. The 41-second job when the app did not change is that cache working
perfectly; the 3½-minute job when it did is the cache being unable to help at
all. See §4 for what a dependency layer recovers.

The tag-triggered `release.yml` builds the `forest` CLI for three targets on
GitHub-hosted runners with **no Rust cache of any kind** — 4 m 28 s, 5 m 45 s and
5 m 51 s in the last release. A `Swatinem/rust-cache` step is a four-line change.

---

## 4. The cheap levers, measured one at a time

Each row is a separate cold `cargo build --workspace` on `apps/forest`, with
everything else held constant. Baseline: **46.3 s, 9.69 GB**.

| Lever | Change | Cold build | Δ time | `target/` | Δ disk |
|---|---|---:|---:|---:|---:|
| **`[profile.dev] debug = 0`** | 2 lines in each `Cargo.toml` | **37.3 s** | **−9.0 s (−19%)** | 3.84 GB | **−5.9 GB (−60%)** |
| `[profile.dev] debug = "line-tables-only"` | 2 lines, keeps backtraces | 39.0 s | −7.3 s (−16%) | 4.36 GB | −5.3 GB (−55%) |
| sccache, cache already warm | `RUSTC_WRAPPER=sccache` | 36.2 s | −10.1 s (−22%) | | |
| sccache, cache empty | first build that populates it | 50.3 s | +4.0 s | | |
| Exclude `ci` (the Dagger crate) | drop one workspace member | 45.1 s | −1.2 s | 7.06 GB | −2.6 GB |
| Exclude `examples/*` | `default-members` | 46.1 s | −0.2 s | 6.73 GB | −3.0 GB |
| Exclude both | | 44.2 s | −2.1 s | 6.33 GB | −3.4 GB |
| `mold` linker | `.cargo/config.toml` | 47.0 s | **none** | | |

And across the two workspaces:

| Lever | Measurement | Result |
|---|---|---|
| Shared `CARGO_TARGET_DIR` | build forest, then forage into the same dir | forage 30.2 s → **28.8 s** (−1.4 s). Combined disk 15.61 GB → **10.95 GB (−4.7 GB)** |
| sccache shared between them | build forest with sccache, then forage | **26% hit rate**, and forage got *slower*: 30.2 s → 33.6 s |

### What each of those actually means

**`debug` is the one real lever, and it is two lines per workspace.** Neither
`Cargo.toml` has a `[profile]` section at all, so every debug build carries full
DWARF: 19% of the wall clock, and 60% of forest's 9.69 GB of `target/`. `debug = "line-tables-only"`
keeps file-and-line in panics and backtraces and gives back most of it; `debug =
0` gives back all of it and costs you debugger support.

**`mold` is already there under a different name.** rustc 1.98 defaults to
`rust-lld` on this target — `readelf -p .comment` on an unmodified
`forest-server` reads `Linker: LLD 22.1.8`. Relinking `forest-server` takes
8.18 s with the default and 8.20 s with mold. The `clang` + `-fuse-ld=mold`
stanza that **both Dockerfiles** write into `/usr/local/cargo/config.toml`, and
the `clang mold` apt install that feeds it, buy nothing. They should come out.

**The examples hypothesis is wrong on time and right on disk.** Excluding the
eleven `examples/` crates from the default build saves 0.2 seconds, because they
are small and compile in parallel with `forest-server`'s 25-second solo run,
which is what the build is actually waiting for. They do cost 3.0 GB. Worth
doing for the disk; do not expect to feel it.

**"Shared dependencies are compiled twice" is true and nearly worthless to
fix.** The two workspaces share 315 exactly-identical name+version packages out
of 645 and 504 — but 87 more names appear in both at *different* versions, and
feature unification differs across the two graphs, so almost none of the shared
315 compile to the same thing. Pointing both at one `CARGO_TARGET_DIR` recovers
1.4 seconds of 30. sccache, which caches at the exact-compilation level and
therefore measures this directly, gets **26%** of forage's compilations from a
cache that forest just filled — and loses more to its own overhead than it wins.
Merging the workspaces would force one version of everything, which is a
dependency-hygiene argument, not a build-speed one. The 4.7 GB of disk is the
real prize here.

**sccache is worth having on CI, not on a laptop.** −22% when warm, +9% when
cold, and it needs somewhere to live. Within one workspace, cargo's own
`target/` already does this job better.

### And the Docker lever, which is the big one for CI

`apps/forage`, locally, base images pre-pulled. Same compile either way; the only
difference is whether the dependency build gets its own layer.

| | Cold (`--no-cache`) | Nothing changed | **One line of Rust changed** | `Cargo.toml` changed |
|---|---:|---:|---:|---:|
| `apps/forage/Dockerfile` today | 54.8 s | 0.8 s | **51.2 s** | same, 51.2 s — `COPY . .` cannot tell the difference |
| With `cargo-chef` | 68.0 s | 1.0 s | **25.8 s** | 52.0 s |

**A source-only commit costs half as much.** The cold build is 13 s slower
because it installs `cargo-chef`, which is a layer that is then cached forever.
The recipe is busted by any manifest or lockfile change, which is **9 of the
last 60 commits (15%)** — so 85% of pushes get the halving.

Scaled to the `32xlarge` CI runner, that is the 3 m 01 s Docker build step
becoming roughly 1½ minutes, on every push that does not touch a manifest.

---

## 5. Bazel: the slice, and what it measured

`MODULE.bazel` + `rules_rust` 0.74.0 + `crate_universe` over `apps/forage` — four
crates, 504 lockfile packages, three test targets, bzlmod throughout, no
`WORKSPACE` file. It builds, and `bazel test //apps/forage/...` runs the same 607
tests cargo runs and passes them.

Getting there took three changes that are the honest shape of what a real
migration costs (all in the commit that added the slice):

- `sqlx::migrate!("src/migrations")` reads a directory at compile time, which a
  sandboxed action cannot see unless it is declared — `compile_data` on
  `forage-db`.
- The minijinja templates live outside the crate's Bazel package, and a `glob`
  cannot cross a package boundary — they had to become a `filegroup`.
- `forage-server`'s tests find those templates through
  `env!("CARGO_MANIFEST_DIR")/../../templates`. rules_rust points that at the
  execroot while `data` lands in runfiles, so 500-odd tests panicked on a
  directory that does not exist. Overriding `CARGO_MANIFEST_DIR` in `rustc_env`
  fixes it without touching application code — but finding that out took a full
  test-log read, and it is the kind of thing every crate needs looking at once.

`apps/forest` would be harder: 274 files of `.sqlx` offline data to declare,
eleven example crates, and a `forest-server` whose acceptance tests do not
currently compile at all.

### The numbers

Same machine, same commit. cargo `dev` ≈ Bazel `fastbuild`; cargo `--release` ≈
Bazel `-c opt`.

| Step | cargo | Bazel | |
|---|---:|---:|---|
| Cold, dependencies already fetched | **30.2 s** | 46.6 s | cargo wins, 1.5× |
| Fresh clone, nothing fetched | 30.2 s + fetch | 58.4 s | |
| No-op rebuild | 0.13 s | 0.17 s | tie |
| One line in a leaf crate | **1.9 s** | 10.2 s | **cargo wins, 5×** |
| One line in a core crate | **2.4 s** | 11.9 s | **cargo wins, 5×** |
| Build + run the tests, first time | 20.8 s | **18.1 s** | Bazel, slightly |
| Re-run the tests, nothing changed | 8.1 s | **0.12 s** | **Bazel wins, 68×** |
| Run the tests after a core change | ≈10.5 s (2.4 rebuild + 8.1 run) | 11.8 s | tie |
| Optimised build, cold | **41.2 s** | 65.2 s | cargo wins, 1.6× |
| Optimised build after a core change | 21.6 s | 22.0 s | tie |
| **Cold build with a populated shared cache** | n/a | **1.1 s** | **Bazel** |
| **Fresh output base + shared cache** | n/a | **13.9 s** | **Bazel** |
| Disk | 5.92 GB `target/` | 2.43 GB output base + 1.09 GB cache | Bazel |

Commands: `bazel build //apps/forage/...`, `bazel test //apps/forage/...`,
`bazel clean` between cold runs, `bazel clean --expunge` for the fresh-clone
rows, `--config=diskcache` (a `--disk_cache` in `.bazelrc`) standing in for a
remote cache.

### Reading those numbers honestly

**The cache hypothesis is confirmed, and it is impressive.** A 46.6-second cold
build becomes **1.1 seconds** when the outputs are already in a shared cache, and
13.9 seconds even after `--expunge` throws away the entire output base. That is
what a remote cache would give a second machine, a CI runner, or a colleague
checking out a branch someone else has built. Nothing cargo can do comes close.
Re-running an unchanged test suite in 0.12 s instead of 8.1 s is the same
mechanism and is genuinely better than cargo, which has no idea the inputs did
not change.

**And the edit loop is five times worse, which is what people feel.** 10–12
seconds instead of 2–2.4 for a one-line change. Bazel cannot use rustc's
incremental compilation — a cacheable action has to be a function of its inputs,
and incremental state is not — so every touched crate recompiles whole. On this
slice it also compiles `forage-server` twice, once as the binary and once as the
test, because the crate has no `lib.rs` to share. Cargo has exactly the same
problem and hides it with incremental compilation. The developer-facing number
gets worse, and it gets worse in the place developers spend all day.

**The cold build is slower too**, by 55% in fastbuild and 58% in opt. Some of
that is sandboxing overhead on ~1,250 actions; some is that rules_rust's default
C toolchain still links with `gold` (it warns about it on every link) where
cargo's rustc uses `rust-lld`.

So: Bazel's case here rests entirely on the 1.1-second cached cold build. The
question is what that is worth, and the answer is *not much in this repository*,
because:

- CI's cold-build problem is already solvable with a Docker layer (§4: 51.2 s →
  25.8 s, no new infrastructure).
- Nobody does cold local builds often. The edit loop is what a working day is
  made of, and Bazel makes it 5× worse.
- The 1.1 s number needs a **remote** cache to help anyone but the machine that
  filled it, and there isn't one. Somebody would have to run and pay for it.

---

## 6. What adopting it would cost

**rust-analyzer.** Works, with a step. `bazel run
@rules_rust//tools/rust_analyzer:gen_rust_project -- //apps/forage/...` produced
a valid `rust-project.json` describing 333 crates, and rust-analyzer consumes it.
But it is a generated 530 KB file that has to be regenerated after every
dependency bump and every BUILD-file edit, it is per-target-pattern rather than
whole-repo, and `cargo` features that editors lean on — `cargo check` on save,
`cargo clippy --fix`, expanding a macro, jumping into a dependency's source —
either need separate wiring or stop working. Everyone in the team would notice
this on day one.

**Contributor friction.** Bazel becomes a required tool: `bazelisk`,
`.bazelversion`, a ~2.4 GB output base per checkout, and a JVM. Every
"just run `cargo build`" instruction in `README.md`, `CLAUDE.md`, and both
`mise.toml`s is now conditionally wrong. New contributors hit Bazel's error
messages, which are not cargo's.

**Dependency updates.** Renovate opens a PR that edits `Cargo.toml` and
`Cargo.lock`. `crate_universe` then has to regenerate, which rewrites
`MODULE.bazel.lock` — **2.0 MB** on this slice, for four crates. Nobody reviews
that diff; it has to be either committed blindly or regenerated in CI and
verified. The repo's `renovate.json` is currently an empty schema stub in
`apps/forest/`, so this would need setting up rather than adjusting.

**Two sources of truth.** Cargo files and BUILD files both describe the build.
`crate_universe` keeps the *dependency versions* in step automatically, which is
the hard half. The rest is manual and silent: a new crate needs a BUILD file, a
new `include_str!` needs `compile_data`, a new integration test directory needs a
target, a changed feature flag needs `crate_features`. None of that fails
loudly — it fails as "cargo builds it and Bazel doesn't", or worse, the reverse.
Nothing in rules_rust checks that for you; keeping the two honest is a review
habit, and review habits decay.

**Does CI actually get faster?** Only with a remote cache, and only for the
Rust compile, which is not the whole job. The build-and-push job is Docker end to
end — it produces an image, pushes it to ghcr, and the deploy resolves it by
digest. Bazel would either need `rules_oci` to produce the image too (another
migration, and it changes what gets pushed), or it would have to hand a binary
to a Dockerfile, at which point the layer caching in §4 is doing the work anyway.
And somebody has to run the cache: `bazel-remote` on a box, or BuildBuddy /
EngFlow at a per-seat price, plus the authentication for it in CI.

---

## 7. Recommendation

### Do this today

Ordered by payoff over effort. The first four are, together, an afternoon.

1. **Fix `apps/forest`'s test suite, then make CI run it.** Two stale
   `examples/`, one stale unit test, four missing `.sqlx` entries. Then add a
   `cargo test --workspace` job to `ci.yaml` — `apps/forage`'s suite runs in
   8.1 s and forest's is not much bigger. This buys zero seconds and is still
   the most valuable thing on this list: right now nothing verifies that the
   Rust in this repository works before it deploys to production.

2. **Add `[profile.dev] debug = "line-tables-only"` to both workspaces.** Two
   lines each. Measured on `apps/forest`: **−16% on every debug build, −5.3 GB
   of `target/`**. Panics and backtraces keep file and line; only a
   step-debugger notices. `debug = 0` is another 3% and another 0.5 GB if
   nobody debugs.

3. **Put a `cargo-chef` dependency layer in both Dockerfiles.** **Halves the
   per-commit CI build** (51.2 s → 25.8 s locally; the 3 m 01 s CI step becomes
   roughly 1½ minutes) for the 85% of commits that do not touch a manifest.
   The whole change, for `apps/forage/Dockerfile`, is this — the compile itself
   is untouched:

   ```dockerfile
   FROM rust:1.98-trixie AS chef
   ENV CARGO_NET_GIT_FETCH_WITH_CLI=true
   RUN apt-get update && apt-get install -y --no-install-recommends \
           git ssh ca-certificates \
       && rm -rf /var/lib/apt/lists/*
   RUN git config --global url."ssh://git@github.com".insteadOf https://github.com && \
       mkdir -p -m 0600 ~/.ssh && ssh-keyscan github.com >> ~/.ssh/known_hosts
   RUN cargo install cargo-chef --locked --version 0.1.72
   WORKDIR /mnt/src

   FROM chef AS planner
   COPY . .
   RUN cargo chef prepare --recipe-path recipe.json

   FROM chef AS builder
   ENV SQLX_OFFLINE=true
   COPY --from=planner /mnt/src/recipe.json recipe.json
   RUN --mount=type=ssh cargo chef cook --release --recipe-path recipe.json
   COPY . .
   RUN --mount=type=ssh cargo build --release -p forage-server
   # ... final stage unchanged
   ```

   `recipe.json` is derived from the manifests alone, so a source-only commit
   leaves it byte-identical and the `cook` layer comes back from the GHA cache
   with all ~500 dependencies already compiled. Note the `clang mold` install
   and the `/usr/local/cargo/config.toml` that configured them are gone, per
   item 4.

4. **Delete the `clang` + `mold` stanza from both Dockerfiles.** It is measurably
   a no-op — rustc already links with `rust-lld` — and it costs an apt install
   of `clang` and `mold` in every image build.

5. **Add `Swatinem/rust-cache` to `release.yml`.** Three uncached release builds
   per tag, 4 m 28 s to 5 m 51 s each. Four lines.

6. **Move `ci` (the Dagger pipeline) out of both workspaces**, or drop it. It is
   invoked by a `mise` task and by nothing else; GitHub Actions does the real CI.
   It costs 1.2 s and 2.6 GB per build, and it is the sole reason `reqwest` 0.11,
   `hyper` 0.14 and `rustls` 0.21 are compiled alongside their modern versions.

7. **Set `default-members` on `apps/forest`** to exclude `examples/*`, so a
   bare `cargo build`/`cargo test` skips them. 3.0 GB of disk; 0.2 s of time.
   Do it for the disk. (`--workspace` still builds them, which is how the two
   rotten examples in item 1 will keep being caught once CI runs the tests.)

8. **Point both workspaces at one `CARGO_TARGET_DIR`** if disk is the pain:
   15.6 GB → 11.0 GB. It is worth 1.4 seconds, so do it for the space or not at
   all.

### This is a project

9. **Split `forest-server`.** 58,512 lines in one compilation unit is 25 of the
   46 seconds of a cold debug build and half of a cold release build, and it is
   on the critical path of every full build in both profiles. It is the
   single largest structural cost in this repository and the only one of these
   items that no tool can fix for you. `forest` (the CLI) at 43,842 lines is the
   same problem one size down.

10. **Reconcile the TLS story.** `aws-lc-sys` costs 17.6 CPU-seconds of C in
    every release build to provide a crypto provider the workspace explicitly
    said it did not want. Getting to ring-only means pinning features across
    `reqwest`, `sqlx`, `tonic` and `hyper-rustls`; worth doing for one provider
    and one audit surface as much as for the seconds.

### Do not do

11. **Bazel.** The slice on this branch works and the measurements are above:
    1.5× slower cold, 5× slower on the edit loop, and its one decisive win — a
    46.6-second cold build served from cache in 1.1 seconds — solves a problem
    that item 3 solves for free, without a remote cache to run, a 2 MB generated
    lockfile in every dependency PR, a degraded IDE, and a second description of
    the build that drifts silently from the first.

    The files stay on this branch as evidence. If the repository changes shape —
    if protobuf or CUE or the frontend ever became real build steps instead of
    committed artifacts, if the crate count doubled, or if someone were already
    running a remote cache for another repo — the arithmetic would be worth
    redoing. Today it is not close.

---

## Variance note

Cold builds were repeated wherever a difference was small enough to be doubted.

| Measurement | Samples | Used |
|---|---|---:|
| `apps/forest` cold debug | 46.3 (instrumented), 46.5 | 46.3 s |
| `apps/forest` cold release | 69.8 (instrumented), 62.9, 63.2, 63.2 | 63.2 s |
| `apps/forage` cold debug | 30.2 (instrumented), 31.1, 30.8 | 30.2 s |
| `apps/forage` cold release | 41.2 (instrumented), 43.0, 41.1 | 41.2 s |

`--timings` costs about 10% on forest's release build and nothing measurable
elsewhere, which is why the release figure moved and the others did not.
Differences under about two seconds in the tables above should be read as noise;
every conclusion drawn from them says so.

The Bazel measurements used `bazel clean` (outputs only, external repositories
kept) as the equivalent of `rm -rf target` with a warm crates registry, and
`bazel clean --expunge` as the equivalent of a fresh clone.
