# 009 - Project Description + Blessed Metadata

**Status:** Phase 1 — Spec, locked. Phase 2 (failing tests) unblocked.
**Depends on:** 004 (Projects + ForestPlatform), 008 (Project as canonical home), task #55 (in-progress `projects.readme`)
**Driver:** A project today carries only `name`, `organisation`, and an in-flight `readme`. The UI has nowhere to show *where the source lives*, *who owns it*, or *where to read the docs* — and those are exactly the questions an engineer lands on the project page asking. GitHub solves this with the right-rail "About" panel (URL, topics, license, releases). We want the same: a small set of blessed metadata fields, set declaratively in `forest.cue`, surfaced on the project Overview and inherited by the component detail.

---

## Problem

The project sidebar currently shows `Created` / `Updated` / `Contracts` and nothing else. Users have no way to:

- Click through to the upstream source repository (GitHub/GitLab URL).
- See which team/person owns the project.
- Find docs, the issue tracker, or a public homepage.
- Filter or group projects by business domain ("payments", "infra", ...).

These fields exist informally — they live in commit messages, Confluence pages, or someone's head. They belong with the project.

A **project-level description** is also missing: today the Overview header shows the *canonical component's* description (folded in by 008). When the project has releases from multiple shapes — or no releases at all — the description disappears or becomes incoherent. The project should carry its own description, independent of any component.

---

## Goals

1. **A small, blessed set of project metadata fields**, defined once in
   `forest.cue` and surfaced in the project Overview sidebar with
   appropriate icons and link-out behaviour.
2. **A project-level description**, set in `forest.cue`, displayed in the
   Overview header as the primary description (with the canonical
   component's description as a fallback so existing projects don't
   regress).
3. **Idempotent publish-time upsert**: `forest publish` reads
   `description` + `metadata` from the CUE file and pushes both to the
   forest server. No separate command. Mirrors the readme upload
   pattern from task #55.
4. **Component inheritance**: the component detail page inherits the
   project's metadata (since the 1:1 project↔component model means
   the metadata is the same). No per-component override in v1.

---

## Blessed fields (v1)

| Field          | Type   | Purpose                                       | Render                       |
| -------------- | ------ | --------------------------------------------- | ---------------------------- |
| `git_url`      | URL    | Upstream source repository                    | Icon + link-out anchor       |
| `homepage`     | URL    | Public landing page / marketing site          | Icon + link-out anchor       |
| `docs_url`     | URL    | Docs site                                     | Icon + link-out anchor       |
| `support_url`  | URL    | Issue tracker / Slack channel / on-call link  | Icon + link-out anchor       |
| `domain`       | string | Business/team domain (e.g. `payments`)        | Icon + text                  |
| `owner`        | string | Responsible team or person (free-form string) | Icon + text                  |

All fields are **optional**. An unpopulated field is omitted from the UI. A project with zero populated fields hides the metadata block entirely (matches 008's empty-state policy).

A separate top-level `description` field (not part of `metadata`) is added in parallel — it sits next to `readme` on the project, not inside the JSONB blob, because it's first-class like `name`.

---

## CUE schema

```cue
#ForestProject: {
    name:         string & =~"^[a-z][a-z0-9-]*$"
    organisation: string & =~"^[a-z][a-z0-9-]*$"
    description?: string
    metadata?:    #ProjectMetadata
}

#ProjectMetadata: {
    git_url?:     string
    homepage?:    string
    docs_url?:    string
    support_url?: string
    domain?:      string
    owner?:       string
}
```

Validation lives on the server (URL parsing, length caps) — CUE keeps a permissive type so users get human-readable errors from the forest server, not opaque CUE complaints.

---

## Storage

One new migration on the forest server adds:

- `description TEXT NOT NULL DEFAULT ''` — parallel to the existing `readme` column from task #55.
- `metadata JSONB NOT NULL DEFAULT '{}'` — holds the blessed fields. JSONB so we can add more blessed keys later without a per-key migration.

Reads always return the full blob; writes always replace it (no partial patch — the source of truth is `forest.cue`, and a publish re-states everything).

---

## gRPC contract

- `Project` message gains `description` (string, tag 4) and `metadata` (ProjectMetadata, tag 5).
- New `ProjectMetadata` message with the 6 typed string fields.
- `UpdateProjectRequest` gains `description` and `metadata` (both optional in the wire sense — empty value clears).
- Already-existing `GetProject` / `UpdateProject` RPCs (from task #55) carry the new fields.

---

## Validation

Server-side, on `UpdateProject`:

- `description`: ≤ 4096 chars. Empty allowed.
- `metadata.*` string fields (`domain`, `owner`): ≤ 256 chars. Empty allowed.
- `metadata.*_url` fields: ≤ 512 chars. **No URL well-formedness check** — these are just links the UI renders verbatim. A malformed value produces a broken link; the user fixes it and re-publishes. Keeping the publish hot-path forgiving is more valuable than rejecting one bad field and blocking a release.

CUE doesn't enforce anything beyond `string` — the forest server is the single source of truth for the length caps so future CLIs (curl, the gRPC bindings) get the same behaviour.

---

## Publish flow

In `forest publish`:

1. Parse `forest.cue` (already happens for `name` + `organisation`).
2. Extract `description` and `metadata` if present.
3. Before the artifact upload, call `update_project_description()` and `update_project_metadata()` with the parsed values (idempotent UPSERT). These are no-ops when values are unchanged.
4. Continue with the existing publish steps.

No new command. Re-stating in CUE on every publish is the contract — config drift between repo and server is avoided.

---

## Forage UI

**`project_detail.html.jinja`:**

- Header: `<p>` description pulls from `project.description` if set, falls back to `summary.description` (canonical component's manifest description) otherwise. Empty when neither has one.
- Sidebar — new "About" block at the **top** (above Components), matching GitHub's repo right-rail layout:
  - One row per populated blessed field with the icon + text/anchor pattern from the table above.
  - Hidden entirely when all six fields are empty.

**`component_detail.html.jinja`:**

- Same description fallback policy as `project_detail`: project description if set, otherwise the component manifest's description. Same text appears on both surfaces when present.
- Same About block at the top of the sidebar (inherited from the parent project).

Both pages share the same render helper — define it once in `templates/components/ui.html.jinja` to avoid drift.

---

## Out of scope

- License field, free-form arbitrary-key metadata, per-component metadata override → explicitly deferred.
- Topics / tags for discovery → out of scope (could become its own future spec).
- Editing metadata through the UI → only via `forest.cue` + publish in v1.
- Validation that `git_url` actually points to a reachable git repo → no live probe.

---

## Testing

- Forest: `release_registry` tests for `update_project_description` (length cap, empty allowed) and `update_project_metadata` (length caps per field, empty allowed, round-trip via `get_project`).
- Forest CLI: integration test that `forest publish` with a CUE file carrying `description` + `metadata` results in the server seeing those values via `get_project`, and that re-publishing without those blocks clears them.
- Forage: route test that the About block appears with populated fields, that link-out anchors carry `rel="noopener noreferrer"` + `target="_blank"`, and that the block is omitted when every field is empty.

---

## Migration story

- Existing projects: `metadata` defaults to `'{}'`, `description` to `''`. No data backfill — projects simply have no About block until their next publish.
- No legacy-fallback for `description` other than the component-description rendering already in place (preserved for backward compat).
