# Policies

Policies are guardrails on releases to an environment. The server enforces them on every path that can deploy there: an explicit release by environment or by destination, a trigger, and each stage of a pipeline.

**Version boundary:** the enforcement below is in server source since **v0.3.17**. Released v0.3.16 checks branch restrictions only for environment-targeted direct releases, not destination-only releases or pipeline stages; its scheduler counts any artifact's success for soak, pipeline soak includes plans, missing source success passes, and policy-evaluation errors can be treated as no policies. Updating the CLI alone does not update the server. Confirm the server revision and independently review branch and same-artifact source exposure before approval.

## Policy types

### Soak time

The **same artifact** must have been successfully deployed to a source environment for at least a duration before it can be deployed to the target environment.

```bash
forest project policy create -o my-org -p my-service \
  --name staging-soak --type soak_time \
  --source-environment staging --target-environment prod \
  --duration 7200
```

- Only a successful **deploy** of that artifact to the source environment counts. A successful plan does not, and neither does another artifact's deploy.
- An artifact never deployed to the source environment has not soaked, so a release of it to the target waits.
- The source and target must differ. A soak policy from an environment to itself could never pass, since nothing would deploy there until it already had, so on the Gitea fork server after dc877a6 creating or updating one is refused, and an existing one reports `can never pass` as its blocker.
- A direct release waits as `QUEUED` and is dispatched once the soak is met. A pipeline stage waits and is re-checked.
- After **v0.3.16**, release status includes the live artifact-specific soak reason for queued steps and ready pending plan/deploy stages, including the missing source environment and required duration. `forest release show <slug>` labels it `blocked:`; a queued prerequisite is not a terminal failure.
- Reasons are derived on each status read, not persisted as execution errors. A successful plan or another artifact's deploy cannot clear this artifact's blocker. Policy changes, assignment and same-artifact successful deploys are reflected on the next read; if evaluation fails, status says policies could not be evaluated rather than implying approval.
- After **v0.3.16**, `release show --follow` refreshes terminal status before rendering its final body, so the original queued blocker is not left on a completed release. An interrupted or prematurely ended follower exits nonzero rather than implying deployment success.

### Branch restriction

Only artifacts annotated from a matching branch can be released to an environment with a branch restriction:

```bash
forest project policy create -o my-org -p my-service \
  --name prod-main-only --type branch_restriction \
  --target-environment prod --branch-pattern '^main$'
```

- The pattern is a regex tested **unanchored**, so anchor it.
- An explicit release (`-e prod` or `-d <a prod destination>`) of a disallowed branch is refused immediately with `blocked by policy '<name>': …`.
- A pipeline plan or deploy stage for that environment **fails** with the same reason, before any plan is offered for approval, and the stages after it are cancelled.
- Queued work rechecks the stored annotation branch against current policies before assignment. If a policy changed to disallow it, that release fails with the same reason; a prior admission or plan approval is not a dispatch exemption.
- After **v0.3.16**, an applicable restriction denies absent, empty or whitespace-only branch metadata, including with a permissive regex. Older servers skipped the restriction when no branch was recorded. The server does not infer a checkout/default branch or rewrite historical annotations. Projects without an applicable branch restriction are unchanged.

### Supersede pending

Older releases still queued for an environment collapse to the newest. After v0.3.18 (server-side), queued plan steps collapse only against other queued plans and queued deploys only against other deploys, so a plan held behind another release's apply is never superseded by that release's deploy. For `forest/terraform@1` destinations it also supersedes an older plan still awaiting approval once a newer plan for the destination starts (with `--same-branch-only`, only one from the same branch; one from another branch is marked stale instead; see [pipelines](pipelines.md)):

```bash
forest project policy create -o my-org -p my-service \
  --name collapse-prod --type supersede_pending \
  --target-environment prod [--same-branch-only]
```

### Approval

The server also has an approval policy, which gates pipeline stages until enough approvals are recorded. The CLI cannot create one. Plan stages with `auto_approve: false` are the CLI-managed approval gate ([pipelines](pipelines.md)).

## Evaluating before you release

```bash
forest project policy evaluate -o my-org -p my-service \
  --target-environment prod --branch main
```

```text
[PASS] prod-main-only (branch_restriction)
       branch 'main' matches pattern '^main$'
[FAIL] staging-soak (soak_time)
       soak time not met: 5400s remaining (1800s since any artifact deployed to staging, 7200s required)
Some policies FAILED for 'prod' — deployment would be blocked
```

This command has no artifact selector: soak is reported against the most recent successful deploy of **any artifact**. It is a partial diagnostic, not release approval: a PASS does not prove the selected artifact has soaked, and an unrelated newer deploy can produce a FAIL. A release checks the artifact actually being released. The command exits 0 whether or not a policy fails, so it is not a CI gate.

## Force

`--force` on a release or trigger only cancels queued releases for the same destinations. It never bypasses a policy.

## If a policy can't be evaluated

When the server cannot evaluate policies, for example on a database error, it fails closed. An explicit release is refused, a trigger does not fire, and a queued release stays queued.

## CLI commands

```bash
forest project policy create   -o <org> -p <project> --name <name> --type <type> [options]
forest project policy list     -o <org> -p <project>
forest project policy update   -o <org> -p <project> --name <name> [options]
forest project policy delete   -o <org> -p <project> --name <name>
forest project policy evaluate -o <org> -p <project> --target-environment <env> [--branch <b>]
```
