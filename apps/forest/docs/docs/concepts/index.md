# Core Concepts

Forest is built around a set of composable primitives. Understanding these concepts is key to using Forest effectively.

## Overview

```
Organisation
 └── Project
      ├── Components (dependencies)
      ├── Environments (dev, staging, prod)
      │    └── Destinations (where to deploy)
      ├── Triggers (auto-release rules)
      ├── Policies (guardrails)
      └── Pipelines (multi-stage DAGs)
```

| Concept | What It Is |
|---------|------------|
| [Project](projects.md) | A service or application managed by Forest |
| [Component](components.md) | A reusable, versioned plugin that provides commands and deployment hooks |
| [Environment](environments.md) | A logical stage (dev, staging, prod) |
| [Destination](destinations.md) | A Forest Runtime target or provider-backed customer platform |
| [Release](releases.md) | An event-sourced operation that moves application code to destinations |
| [Pipeline](pipelines.md) | A multi-stage deployment DAG |
| [Trigger](triggers.md) | An automatic release rule based on patterns |
| [Policy](policies.md) | A guardrail that gates releases |

## How They Fit Together

1. A **project** declares **component** dependencies in `forest.cue`
2. Components provide deployment logic and configuration schemas
3. **Environments** organize your deployment stages
4. **Destinations** select Forest Runtime or a deployment provider for
   customer-owned infrastructure
5. A **release** prepares application artifacts, records source and actor
   context, applies policy, and schedules matching destinations
6. **Triggers** can automate releases based on commit patterns
7. **Policies** enforce rules (soak time, branch restrictions, approvals) before a release proceeds
8. **Pipelines** orchestrate multi-stage rollouts across environments
