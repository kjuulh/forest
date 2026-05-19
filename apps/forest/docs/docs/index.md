# Forest

**Composable workflow and release management for teams.**

Forest helps you design shareable development workflows, compose services from reusable components, and manage production releases with policies, triggers, and pipelines.

---

## What is Forest?

Forest is a platform that brings structure to how teams build, deploy, and operate software. Instead of gluing together ad-hoc scripts and CI/CD configs, Forest provides a component model where:

- **Components** are self-contained, versioned plugins (Rust, Go, or Docker) that define commands, deployment hooks, and configuration schemas using [CUE](https://cuelang.org/).
- **Projects** compose components together, providing environment-specific configuration.
- **Releases** are first-class, event-sourced operations with full lifecycle management — from annotation through deployment to rollback.
- **Policies** enforce guardrails like soak times, branch restrictions, and approval gates.
- **Triggers** automate releases based on branch patterns, commit metadata, or source types.
- **Pipelines** orchestrate multi-stage deployments as directed acyclic graphs (DAGs).

## Key Features

| Feature | Description |
|---------|-------------|
| **Component registry** | Publish and share reusable components across teams |
| **CUE-based configuration** | Type-safe, composable configuration with schema validation |
| **Event-sourced releases** | Full audit trail with status tracking and real-time streaming |
| **Release pipelines** | Multi-stage DAG deployments (deploy, wait, plan) |
| **Policy engine** | Soak time, branch restriction, and external approval policies |
| **Automated triggers** | Pattern-based auto-release on commits, branches, and PRs |
| **Multi-environment** | First-class support for dev, staging, prod (and custom environments) |
| **Distributed runners** | Execute deployments on remote infrastructure |
| **Lock files** | Reproducible dependency resolution with `forest.lock` |

## How It Works

```
forest.cue (project config)
    |
    v
+-- Components (versioned plugins) --+
|   kubernetes-service                |
|   terraform-service                 |
|   docker-builder                    |
+-------------------------------------+
    |
    v
Environments (dev / staging / prod)
    |
    v
Destinations (where to deploy)
    |
    v
Releases (event-sourced lifecycle)
    |
    +-- Triggers (auto-fire on patterns)
    +-- Policies (guard with rules)
    +-- Pipelines (multi-stage DAGs)
```

## Quick Links

- [Quickstart](quickstart.md) — Get running in 5 minutes
- [Getting Started](getting-started/index.md) — Step-by-step setup guide
- [Concepts](concepts/index.md) — Understand the core model
- [CLI Reference](reference/cli.md) — Full command reference
- [Authoring Components](guides/authoring-components.md) — Build your own components
