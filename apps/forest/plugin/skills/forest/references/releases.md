# Releases, environments and destinations

Default to the pipeline-first flow in [pipelines-and-policies.md](pipelines-and-policies.md). This file explains the pieces that flow is built from, and the explicit commands for when the user asks for one environment specifically.

## The model

- **Environment**: a logical stage (`dev`, `prod`), organisation-wide. Inspect with `forest environment list -o <org>`.
- **Destination**: a concrete place in one environment, such as an account, region or cluster, with a **type** that decides how it deploys (`forest/terraform@1`, `forest/generic@1`, `forest/flux@1`, `forest/kubernetes@1`, `forest/noop@1`, …). Destinations are shared by every project there, so they name the place, not the service. Inspect with `forest destination list -o <org>` and `forest destination types`.
- **Project → destinations** is declared in `forest.cue`. `destination` is a **regex** over destination names:

```cue
dependencies: "forest/deployment": version: "<published version>"   // the deployment contract; prepare refuses to run without it

project: env: prod: {
	destinations: [{destination: "^prod/.*$", type: "forest/generic@1"}]
	config: service: "my_service"          // merged over the destination's metadata; the project wins
}
// or, with a component that renders manifests:
"my-org": "terraform-service": env: prod: {
	destinations: [{destination: "^infrastructure-prod/", type: "forest/terraform@1"}]
	config: replicas: 3
}
```

An unanchored pattern matches more than intended: `prod` matches `preprod-eu`. Anchor it (`^prod/.*$`), and compare it with `forest destination list` before the first release.

## The three steps

| Step | Command | What it does |
|---|---|---|
| prepare | `forest release prepare [--set k=v]` | Resolves dependencies, **deletes and rewrites** `.forest/deployment/`, and calls component `prepare` hooks. It renders every environment, not only the target. Local only. |
| annotate | `forest release annotate --context-title "<t>" [--source-type ci] [--detect]` | Uploads `.forest/deployment/` from the current directory plus the spec, and records an immutable annotation with git context. Prints a **slug** (e.g. `deadly-amusing-asp`). **Evaluates triggers** unless `--annotation-only` is passed. |
| release | `forest release release <slug> -e <env> \| -d <dest>… [--pipeline]` | Creates a release intent for that artifact. Only one release can be in flight per project and destination; `--force` cancels queued ones (it does not bypass policies). |

`forest release create -e <env>` does all three for one environment, with triggers skipped (unless `--skip-commit` is supplied). With `--pipeline` it instead runs the whole pipeline. Since v0.3.17, `--environment` is optional: a no-target `create` prepares and annotates with triggers enabled, **which may deploy prod**. Check `create --help`, not a local build's version string. Commit first: the annotation records git context, not proof of the running code.

Useful flags:

- `--set <org>/<component>.<key>=<v>` sets a component's config, and `--set config.<key>=<v>` sets a generic destination's block. Use it for values only CI knows, such as an image tag. Overrides are recorded on the annotation. `--set config.<key>` reaches every environment's block. Once its `stable` branch includes it, `service-release.yml` takes a `set` input: one `--set` value per line, trimmed and passed as one argument (not for secrets: values are logged and recorded on the annotation); detect it by that input in the workflow. Its older `image-tag` input sets only a component's `image.tag`, so a generic destination's ECS `pin_image` needs `set: config.image_tag=<tag>`. A provider gets no commit SHA from a generic destination, so `image_tag_template` with `{sha}` fails there.
- `--source-type ci` matches triggers configured for `^ci$`. Without it, CI may be detected or `local` used; inspect the actual filters rather than assuming a laptop annotation cannot deploy.
- `--detect` attributes the release to the commit author in CI rather than the token's owner.
- `--no-wait` returns once the intent is staged. `--no-health` skips the post-release health watch, and "Health watch timed out" still exits 0.
- `release release --pipeline` retains the environment-selection prompt unless a target is supplied. Use `-e <env>` in noninteractive commands, but it does not fence the pipeline to that environment; inspect every stage.

## Promotion and rollback

Both are re-releasing an **existing** artifact; nothing is rebuilt:

```bash
forest release release <slug> -o <org> -p <project> -e prod
```

- Prefer pipeline promotion: an artifact that passed `deploy-dev` proceeds through the plan approval to `deploy-prod`. Review same-artifact dev success, elapsed time and branch before approval; runtime policy gaps are in [pipelines-and-policies.md](pipelines-and-policies.md). A manual `-e prod` skips pipeline ordering and plan approval.
- A rollback is the same command with the **previous good** slug. Find it in CI logs or the Forage UI, or with `forest release` on a terminal, whose picker marks the latest succeeded release per destination. There is no rollback command, and component `rollback` hooks are not invoked by the CLI.
- Since v0.3.17, `release release` and its `release` shorthand reject `--ref` / `-r` during argument parsing, before release side effects. Older CLIs accepted but ignored them. Select an existing artifact by its annotation slug or `--artifact-id <artifact-uuid>`; a git branch is not an artifact identity.
- `-d <destination>` releases to named destinations. A server with the v0.3.17 change checks branch restriction for their environments; an older one does not. Never use `-d` to get around a policy.

## Observing a release

Use a snapshot to read a parked plan; `--follow` intentionally waits through approval until the release is terminal.

Since v0.3.17, a follower reads the final intent state before rendering its body, so a completed release does not retain its initial queued blocker. Follow stream errors or a stream ending before a terminal outcome exit nonzero; they are not deployment success. Snapshot mode retains its existing persisted-history fallback.

```bash
forest release show <slug|intent-uuid> -o <org> -p <project>
forest release show <slug> --follow
forest release show <slug> --logs-only > release.log
forest project releases -o <org> -p <project> [--all]
```

How to read `release show`:

- **`stages:`** exists only for pipeline releases. Each stage is `PENDING`, `ACTIVE`, `AWAITING_APPROVAL`, `SUCCEEDED`, `FAILED`, `CANCELLED` or `SUPERSEDED`. A plan stage prints the approve command when it parks; after v0.3.18 a plan a newer one replaced shows `[ACTIVE, STALE]` with the reason instead, and no approve command.
- **`destinations:`** lists one row per step, and a plan step counts. During an approval wait you can see `✓ [prod] prod/eu [SUCCEEDED]` while `deploy-prod` is still `PENDING`; that row is the plan. After the deploy, `prod/eu` appears twice. Decide "deployed" from the **deploy** stage.
- Steps go `QUEUED → ASSIGNED → RUNNING → SUCCEEDED | FAILED | TIMED_OUT | CANCELLED | SUPERSEDED`. A step left `QUEUED` is usually held by a soak-time policy or by another in-flight release for that destination. After v0.3.18 (server-side, no CLI flag), a terraform plan step also stays `QUEUED` while another release of the project has an earlier plan for that destination still queued or running, or an approved saved plan (under 7 days old, and not "No changes") that its deploy has not yet applied. `release show` and `project releases` then print `waiting: waiting for release <slug> to apply its plan for <destination> before planning` under the stage, and the log has the same line.
- `no release intent found for slug '<slug>'`: the annotation exists but nothing was released. No trigger matched (often a laptop annotation without `--source-type ci`), or it was a `create --skip-commit` on a CLI without the annotate-for-triggers change.
- Exit codes: `show` exits 1 if a step failed, timed out or was cancelled. Since v0.3.17 it also exits 1 when a pipeline stage failed or was cancelled, for example blocked by a branch policy; on v0.3.16 and older such a release exits 0, so read the stage lines. It exits 0 while queued, running or awaiting approval. `release`/`create` exit 0 when parked for approval or with `--no-wait`, and 1 when a destination failed.

`forest project releases` renders a plan stage as `deploy(<env>)`; trust `release show` for stage types. Neither command shows the image or SHA running in the service.

In the fork's Forage UI after v0.3.18, individual releases have a high-resolution
**Release pipeline** workspace below the header. Overviews have one global
**Environment status** workspace with readable environment lanes: a compact
environment card sits above rows of destination cards, wrapping after four
destinations. Shared connectors retain logical environment-to-destination edges.
Each physical target has one card, not one per operation. Expanding an active
timeline entry lazily loads its pipeline. Collapsed or initially inactive entries
make no pipeline graph request. Original timelines and stage disclosures remain;
there is no lower-resolution graph.
Pipeline nodes separate stages, destination runs, approvals and policies. Their
camera focuses active work without filtering completed or future nodes away.
Desktop global overviews start with **Fit** for the whole workspace; **Active**
frames the active environment and its destinations. Fullscreen expands the
workspace. Drag to pan, scroll to zoom, or use F/A. Select a node to pin inspection.
Overview Plan text loads only when Plan opens. The global view has no Plan/Logs.
Detail approval controls reuse existing forms and CSRF fields; overview controls
link to the release. Details poll every ten seconds while unfinished and visible.
Canonical overview requests start at most once per ten seconds while visible;
only expanded visible pipeline fragments refresh. Pinned/manual inspection survives.
Each target card retains current completed and incoming context, with a pending
count when needed. The inspector keeps every operation's provenance, state,
reason and timing. A newer completion never hides queued work; pinned selection
follows the physical target across operation changes. A successful plan is not
deployed; missing provenance is explicit. None of these views proves the image
serving.

Final readability browser verification of the revised lanes and current/incoming
cards is pending. Earlier workspace checks do not prove the revised layout's
framing, wrapped-card readability or mobile overflow behavior.

## Proving a deployment

A green release says the destination provider reported success. That is good evidence for `terraform` (the apply succeeded) and for providers that wait for convergence. It still is not proof that the new code is serving. Close the loop with what the service itself reports: the running image digest or build SHA on a health/version endpoint, the ECS task definition, the pod image, or the `service_version` stamped on telemetry. Compare it with the commit on the annotation (`source:` in `release show`). Say plainly which of these you checked.

`forest release fail <slug> -r "<reason>" -e <env>` records that a deploy announced by an annotation never happened, when something outside Forest did the deploying.

## Terraform destinations

- `forest release drift -e <env> [-d <dest>] [-q]` runs a refresh-only plan against the last successful artifact. It changes nothing. Through v0.3.18 it exits 0 when in sync, 2 on drift and 1 when the check couldn't run; a destination type without a drift check counts as in sync, and drift outranks a failed check, so 0 does not mean every destination was checked. After v0.3.18 it exits 0 only when every destination was checked and is in sync, 2 on drift with no failed check, and 1 when any result is empty, unsupported or failed, which outranks drift elsewhere. Tell them apart by the exit-status line in `forest release drift --help`: the newer one says "every destination checked".
- `forest state list|versions|pull|unlock|env -e <env>` reaches the state outside a release. It needs **Admin**, every call is audited, and `pull`/`env` expose secrets. Use these only when the user asks, and never paste the output.
- State locks (forest-server after v0.3.18; detect: `forest state versions` prints a `holder:` line under `LOCKED by`): the backend records each lock's operation, who and holder (in-process release and replica, remote runner, or the user/app whose `state env --write` credential took it). Forest clears a lock only when its holder is known dead: an in-process release's own lock once its terraform exited, or, before a plan/apply, the lock of an ended release whose replica shut down cleanly (marked stopped) or has been silent for `FOREST_REPLICA_GONE_SECS` (default 1200), or a lock with no recorded holder older than 60 minutes while nothing else runs on the destination. Runner, user and app locks and live replicas' locks are only reported in the release log. The reaper never removes locks. Older servers record only the lock ID.
- Plan approval applies exactly the saved plan. If state moved since, the deploy fails as stale; re-run the release and review the new plan. After v0.3.18 (server-side) a later release's plan waits for an earlier release's in-flight plan, or its approved, unapplied plan that changes something, at the same destination; a rejected, cancelled, superseded, applied, no-op or 7-day-old plan never holds it. A plan still awaiting approval does not hold a newer one: when the newer plan starts, the older is superseded (a `supersede_pending` policy covers the environment, and with `same_branch_only` the branch matches) or marked stale (`[ACTIVE, STALE]` in `release show`; approve is refused; reject it, then release again for a fresh plan, since releasing the same artifact while it is active returns the stale release). Its saved plan is deleted either way.

## Failure modes

| Symptom | Cause | Next step |
|---|---|---|
| `the 'forest/deployment' contract is not in your dependencies` | prepare needs the deployment contract | add `"forest/deployment": version: "<published>"` (`forest components show forest/deployment`) |
| `blocked by policy '<name>': branch '…' does not match …` | branch_restriction on the target environment | release a build from an allowed branch; don't route around it with `-d` |
| `branch metadata is absent or empty` (since v0.3.17) | target branch restriction requires recorded provenance | produce a fresh annotation with the actual source branch; do not invent it from a default/current branch or edit historical annotations |
| `interrupted by forest-server shutdown` (servers after v0.3.18; older ones say `release heartbeat lost`) | the server was restarted and the release outlived its drain window (`FOREST_DRAIN_TIMEOUT_SECS`, default 90s); it is failed, never re-queued | check the destination, then re-run the release. For terraform, servers after v0.3.18 release the interrupted release's lock once terraform is stopped; on older ones, if the next release reports the state locked, `forest state unlock` (Admin, ask first) |
| `Error acquiring the state lock: HTTP remote state already locked` | another run holds the terraform state; after v0.3.18 the Lock Info `Who` names the holder (`forest release <id> on <replica>`, a runner, or `user:<id>`) | `forest state versions -e <env>` (Admin) shows the holder and its release status, and the release log says why the lock was kept. If that release is running or its replica alive or only silent, wait. A person's or runner's lock is never cleared automatically: ask them, or `forest state unlock` (Admin, ask first) |
| step stays `QUEUED` | soak_time not met, or another release in flight | since v0.3.17, read the artifact-specific `blocked:` reason in `release show`; inspect its successful source deploy and elapsed time. `policy evaluate` remains partial any-artifact diagnostics, not approval |
| `no release intent found for slug` | annotation with no release | check triggers and `--source-type`; see [pipelines-and-policies.md](pipelines-and-policies.md) |
| `The input device is not a TTY` | a picker or prompt needs `-o`/`-p`, a slug or other flags | pass them explicitly |
| `failed to find upstream component` during prepare | a dependency version is a range or unpublished | pin an exact published version ([components.md](components.md)) |
| `TimedOut` | runner never started (5 min) or ran over an hour | check the provider and destination metadata; release again |
