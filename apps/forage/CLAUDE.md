# Forage Client - AI Development Guide

## Project Overview

Forage is the managed platform and registry for [Forest](https://src.rawpotion.io/rawpotion/forest) - an infrastructure-as-code tool that lets organisations codify their development workflows, CI, deployments, and component sharing. Forage extends forest by providing:

- **Component Registry**: Host and distribute forest components
- **Managed Deployments**: Push a `forest.cue` manifest and get automatic deployment (Heroku-like experience)
- **Container Runtimes**: Pay-as-you-go alternative to Kubernetes
- **Managed Services**: Databases, user management, observability, and more
- **Organisation Management**: Teams, billing, access control

## Architecture

- **Language**: Rust
- **Web Framework**: Axum
- **Templating**: sedge (typed templates compiled to Rust, server-side rendered; git.kjuulh.io/kjuulh/sedge)
- **Styling**: Tailwind CSS (via standalone CLI)
- **Database**: PostgreSQL (via SQLx, compile-time checked queries)
- **Build System**: Forest + Mise for task running

## Project Structure

```
/
├── CLAUDE.md                  # This file
├── Cargo.toml                 # Workspace root
├── forest.cue                 # Forest project manifest
├── mise.toml                  # Task runner configuration
├── crates/
│   ├── forage-server/         # Main axum web server
│   │   ├── src/
│   │   │   ├── main.rs
│   │   │   ├── routes/        # Axum route handlers
│   │   │   ├── views/         # Page view models and the generated sedge module
│   │   │   └── state.rs       # Application state
│   │   ├── templates/pages/   # One .html.sedge per page
│   │   └── Cargo.toml
│   ├── forage-ui/             # Shared sedge components and layouts (Shell, ProjectSettings, OrgSettings)
│   │   ├── templates/         # components/*.html.sedge, layouts/*.html.sedge
│   │   └── src/               # Typed props (Tone, Chrome, NavItem…) and formatting
│   ├── forage-core/           # Business logic, pure functions
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── registry/      # Component registry logic
│   │   │   ├── deployments/   # Deployment orchestration
│   │   │   └── billing/       # Pricing and billing
│   │   └── Cargo.toml
│   └── forage-db/             # Database layer
│       ├── src/
│       │   ├── lib.rs
│       │   └── migrations/
│       └── Cargo.toml
├── static/                    # Static assets (CSS, JS, images)
├── specs/                     # VSDD specification documents
└── tests/                     # Integration tests
```

## Development Methodology: VSDD

This project follows **Verified Spec-Driven Development (VSDD)**. See `specs/VSDD.md` for the full methodology.

### Key Rules for AI Development

Follow the VSDD pipeline **religiously**. No shortcuts, no skipping phases.

1. **Spec First**: Never implement without a spec in `specs/`. Read the spec before writing code.
2. **Test First**: Write failing tests before implementation. No code exists without a test that demanded it. Confirm tests fail (Red) before writing implementation (Green).
3. **Pure Core / Effectful Shell**: `forage-core` is the pure, testable core. `forage-server` is the effectful shell. Database access lives in `forage-db`.
4. **Minimal Implementation**: Write the minimum code to pass each test. Refactor only after green.
5. **Trace Everything**: Every spec requirement maps to tests which map to implementation.
6. **Adversarial Review**: After implementation, conduct a thorough adversarial review (Phase 3). Save reviews in `specs/reviews/`.
7. **Feedback Loop**: Review findings feed back into specs and tests (Phase 4). Iterate until convergence.
8. **Hardening**: Run clippy, cargo-audit, and static analysis (Phase 5). Property-based tests where applicable.

## Commands

- `mise run develop` - Start the dev server
- `mise run test` - Run all tests
- `mise run db:migrate` - Run database migrations
- `mise run build` - Build release binary
- `forest run <command>` - Run forest-defined commands

## Conventions

- Use `snake_case` for all Rust identifiers
- Prefer `thiserror` for error types in libraries, `anyhow` in binaries
- All database queries use SQLx compile-time checking
- Pages are sedge templates (`crates/forage-server/templates/pages/*.html.sedge`) rendering a typed view model from `src/views/`; anything two pages repeat is a forage-ui component. No inline `<script>`/`<style>`/`on*=` (sedge rejects them): behaviour goes in `static/js/*.js` loaded via `state.templates.asset_href` in the Shell's `scripts` slot, confirms use `data-confirm`, copy buttons `data-copy`. Every page has route tests and an insta snapshot of `<main>`.
- Routes are organized by feature in `routes/` modules
- All public API endpoints return proper HTTP status codes
- Configuration via environment variables with sensible defaults
- **Forms with conditional sections**: When a form has multiple sections toggled by a dropdown (e.g. policy type), inputs in hidden sections **must be disabled** so they are excluded from submission. Duplicate `name` attributes across sections cause axum's form deserializer to fail with "unsupported value". Always call the toggle function on page load to disable hidden inputs from the start.
- **Tests live in separate files**, never inline in the main source file:
  - Unit tests for private functions: `#[cfg(test)] mod tests` in the same file (e.g., `forest_client.rs`)
  - Route/integration tests: `src/tests/` directory with files per feature area (e.g., `auth_tests.rs`, `platform_tests.rs`)
  - Mock infrastructure and test helpers: `src/test_support.rs` (`pub(crate)` items)
  - Keep production source files clean - no test code bloat
