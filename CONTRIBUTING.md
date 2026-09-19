# Contributing to Forest

Forest is currently a private project. Contributions require repository access and must preserve the separation between the generic Component Exchange and private deployment overlays.

## Before changing code

- Read the [product direction](apps/forest/docs/docs/product/index.md).
- Treat [security and sandboxing](apps/forest/docs/docs/product/security-and-sandboxing.md) as a contract, not future marketing copy.
- Do not introduce a second component, registry, authentication, or configuration convention beside an existing one.
- Keep generic product code free of organisation hosts, account IDs, catalogues, credentials, and deployment state.
- Put private Woodpecker and deployment integration changes in their existing overlay paths.

## Development setup

```bash
git clone git@git.kjuulh.io:kjuulh/forest.git
cd forest/apps/forest
mise install
cargo build --locked --workspace
```

Start local dependencies when working on `forest-server`:

```bash
mise run local:up
mise run dev
```

The Compose stack provides PostgreSQL, NATS, and MinIO for development only.

For Forage web development:

```bash
cd apps/forage/frontend
npm ci
npm test
npm run build
```

## Change rules

### Components and CLI

- `forest build` was removed. Components build through a depended-on build component with `forest run build`.
- Run `forest publish --dry-run` before testing a real publication.
- Published component versions are immutable; use a new version instead of overwriting one.
- A component manifest or protocol change must update all SDKs, generated types, examples, and compatibility documentation together.
- Never weaken checksum verification, lock-file reproducibility, or authorization to make an example pass.

### SQLx

`forest-server` release builds use checked-in offline query metadata. After changing a compile-time SQL query:

```bash
mise run local:up
mise run db:prepare
```

Commit the resulting changes under `apps/forest/crates/forest-server/.sqlx/`. Run preparation from the server crate through the mise task; workspace-level preparation writes the cache to the wrong directory.

### Generated files

Regenerate rather than hand-edit:

- protobuf/gRPC outputs;
- SDK code generated from CUE;
- Forage frontend bundles and CSS;
- SQLx offline query metadata.

Keep generated changes in the same commit as their source change.

### Documentation

- Commands in documentation must match the compiled `forest --help` surface.
- Label planned features explicitly. Do not present roadmap items, pricing, isolation, SLAs, or compliance as shipped.
- Use neutral example organisations such as `acme`; private integration examples must be clearly labelled.
- Update the root README, MkDocs navigation, and affected reference pages when changing a user-facing command or contract.

## Verification

Use the narrowest proof that exercises the changed surface.

Forest workspace:

```bash
cd apps/forest
cargo test --locked --workspace
```

The server acceptance suite needs the local dependency stack and environment documented in [`apps/forest/README.md`](apps/forest/README.md).

Forage Rust and frontend:

```bash
cd apps/forage
cargo test --locked --workspace

cd frontend
npm ci
npm test
npm run test:bundle
```

Documentation:

```bash
cd apps/forest/docs
python -m pip install -r requirements.txt
mkdocs build --strict
```

Tests must assert consumer-visible behaviour, boundaries, invariants, transitions, precedence, or real errors. Avoid tests that only pin copy, field forwarding, source text, or mock plumbing.

## Security-sensitive changes

For authentication, authorization, artifact handling, runner execution, templates, OAuth, webhooks, or secrets:

- document the trust boundary and failure mode;
- include negative authorization or isolation coverage;
- verify logs and errors do not expose secrets;
- preserve deny-by-default behaviour;
- request focused security review before merge.

Never commit `.env`, tokens, private keys, production host credentials, customer data, or sanitized-looking copies of real secrets. If a secret reaches Git, rotate it; deleting the line is not remediation.

Report suspected vulnerabilities through [`SECURITY.md`](SECURITY.md), not an ordinary issue.

## Review and merge

- Use a focused branch and a descriptive commit.
- Explain observable behaviour, risk, migration, and verification in the pull request.
- Keep generic product changes separate from private deployment changes.
- Do not force-push shared or protected branches.
- A reviewer must be able to reproduce the changed path without access to an author's untracked files.

## Licence

No public licence has been selected. A contribution does not imply permission to redistribute the repository. Contributor and relicensing terms must be resolved before accepting public contributions.
