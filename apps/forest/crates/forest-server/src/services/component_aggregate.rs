use std::pin::Pin;

use anyhow::Context;
use forest_event_store::EventStore;
use futures::{SinkExt, Stream};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::domains::component::{self, ComponentAggregate};

/// Render a [`forest_manifest::ComponentShape`] as the lowercase string we
/// persist in `components.shape` (see migration `20260514000000_component_shape.sql`).
fn shape_to_str(s: forest_manifest::ComponentShape) -> &'static str {
    match s {
        forest_manifest::ComponentShape::Component => "component",
        forest_manifest::ComponentShape::HybridComponent => "hybrid_component",
        forest_manifest::ComponentShape::ToolBinary => "tool_binary",
        forest_manifest::ComponentShape::ToolExternal => "tool_external",
    }
}

/// Convert the lowercase `components.shape` string back to the proto enum.
fn shape_str_to_proto(s: &str) -> forest_grpc_interface::ComponentShape {
    match s {
        "component" => forest_grpc_interface::ComponentShape::Component,
        "hybrid_component" => forest_grpc_interface::ComponentShape::Hybrid,
        "tool_binary" => forest_grpc_interface::ComponentShape::ToolBinary,
        "tool_external" => forest_grpc_interface::ComponentShape::ToolExternal,
        _ => forest_grpc_interface::ComponentShape::Unspecified,
    }
}

/// Order two component versions by semver precedence, ascending (DATA-583).
///
/// Replaces a SQL `ORDER BY split_part(version, '.', N)::int`, which was not
/// just imprecise but *fatal*: the third dot-part of `0.1.7-ci.1` is `7-ci`,
/// and casting that to `int` raises `invalid input syntax for type integer`.
/// A single prerelease anywhere in a component's history therefore broke every
/// query using that ordering, taking `forest components show` down for the
/// whole component.
///
/// Non-semver versions can exist in old rows, so parse failures must not panic
/// or reorder anything violently: anything unparseable sorts below everything
/// parseable, and ties fall back to a plain string comparison so the order
/// stays total and stable.
fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    match (semver::Version::parse(a), semver::Version::parse(b)) {
        (Ok(a), Ok(b)) => a.cmp(&b),
        (Ok(_), Err(_)) => std::cmp::Ordering::Greater,
        (Err(_), Ok(_)) => std::cmp::Ordering::Less,
        (Err(_), Err(_)) => a.cmp(b),
    }
}

/// The version a component should report as its latest: the highest **stable**
/// one, falling back to the highest of any kind when no stable release exists.
///
/// Ordering alone is not enough. A prerelease sorts below *its own* release but
/// still above the previous one, so `0.2.0-rc.1` outranks a shipped `0.1.8` and
/// would present itself as the component's latest version — a release candidate
/// promoting itself on the detail page.
///
/// The fallback keeps a component that has only ever published release
/// candidates from reporting no version at all.
///
/// Same rule fungus applies to ports (DATA-499), the same one `list_org_tools`
/// already used, and the same one the CLI's `VersionSpec::resolve` uses so an
/// install and the page describing it agree.
fn pick_latest_version(versions: &[String]) -> Option<&String> {
    versions
        .iter()
        .filter(|v| !is_prerelease(v))
        .max_by(|a, b| compare_versions(a, b))
        .or_else(|| versions.iter().max_by(|a, b| compare_versions(a, b)))
}

// ============================================================
// Read-model types
// ============================================================

pub struct ComponentVersion {
    pub id: String,
    pub name: String,
    pub organisation: String,
    pub version: String,
}

pub struct ComponentVersionInfo {
    pub version: String,
    pub protocol_version: String,
    pub kind: String,
    pub platforms: Vec<String>,
    /// RFC 3339 timestamp of when this version was published (the `created`
    /// column, set at commit time).
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct OrgToolRow {
    pub organisation: String,
    pub name: String,
    pub latest_version: String,
    pub shape: String,
    pub tool: Option<ToolFacetRow>,
    pub upstream_host: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ToolFacetRow {
    pub name: String,
    pub argv_passthrough: bool,
    pub description: Option<String>,
}

/// Best-effort prerelease detector — matches `<digits>.<digits>.<digits>-<anything>`.
fn is_prerelease(v: &str) -> bool {
    let core = v.split('+').next().unwrap_or(v);
    core.contains('-')
}

/// Compare two semver-shaped versions lexically on `(major, minor, patch)`.
fn version_gt(a: &str, b: &str) -> bool {
    fn parts(v: &str) -> [u64; 3] {
        let core = v.split(['-', '+']).next().unwrap_or(v);
        let mut p = [0u64; 3];
        for (i, segment) in core.split('.').enumerate().take(3) {
            p[i] = segment.parse().unwrap_or(0);
        }
        p
    }
    parts(a) > parts(b)
}

/// Extract the host component from an https:// URL.
fn extract_host(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let host = rest.split('/').next()?;
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

/// Load the README markdown for a component at a specific version.
///
/// Looks up the `component_files` projection for `README.md` (or
/// case-insensitive variant) belonging to that version, then fetches the
/// bytes from S3 and returns them as a UTF-8 string. Returns
/// `Ok(String::new())` when no README is found — the caller decides
/// whether that's an error or just an empty response.
async fn load_component_readme(
    db: &sqlx::PgPool,
    object_store: &crate::object_store::ObjectStore,
    organisation: &str,
    name: &str,
    version: &str,
) -> anyhow::Result<String> {
    use sqlx::Row;
    // Look up the component_id matching this org/name/version, then find a
    // README.md file path under it. The path is the in-archive name; the
    // S3 key is derived from (org, name, version, path).
    let row = sqlx::query(
        "SELECT cf.file_path
         FROM component_files cf
         JOIN components c ON c.id = cf.component_id
         WHERE c.organisation = $1
           AND c.name = $2
           AND c.version = $3
           AND lower(cf.file_path) IN ('readme.md', 'readme', 'readme.markdown')
         ORDER BY lower(cf.file_path)
         LIMIT 1",
    )
    .bind(organisation)
    .bind(name)
    .bind(version)
    .fetch_optional(db)
    .await
    .context("query component_files for README")?;

    let file_path: String = match row {
        Some(r) => r.get("file_path"),
        None => return Ok(String::new()),
    };

    let s3_key = crate::object_store::keys::component_file(organisation, name, version, &file_path);
    // 5s timeout — README is on the detail page hot path; a stalled S3
    // request must not block the whole page render. 1 MiB cap on the
    // decoded text — the gRPC write boundary caps writes at 64 KiB, but
    // older objects predating that cap could still be larger.
    const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
    const MAX_README_BYTES: usize = 1024 * 1024;
    let fetch = object_store.get(&s3_key);
    let bytes = match tokio::time::timeout(READ_TIMEOUT, fetch).await {
        Ok(Ok(b)) => b,
        Ok(Err(e)) => {
            // Don't fail the whole detail response if the README blob is
            // missing — log + return empty. The detail page degrades to
            // no README, which is the same as "never uploaded one".
            tracing::warn!("README S3 fetch failed for {organisation}/{name}@{version}: {e:#}");
            return Ok(String::new());
        }
        Err(_) => {
            tracing::warn!(
                "README S3 fetch timed out (>{READ_TIMEOUT:?}) for {organisation}/{name}@{version}"
            );
            return Ok(String::new());
        }
    };

    let truncated: &[u8] = if bytes.len() > MAX_README_BYTES {
        tracing::warn!(
            "README at {organisation}/{name}@{version} exceeds {MAX_README_BYTES} bytes ({} actual); truncating",
            bytes.len()
        );
        &bytes[..MAX_README_BYTES]
    } else {
        &bytes
    };

    // README is markdown text; surface as UTF-8 string. Lossy decode
    // handles malformed bytes without panicking.
    Ok(String::from_utf8_lossy(truncated).into_owned())
}

// ============================================================
// Service — orchestrates aggregate + projections
// ============================================================

/// In-flight upload metadata resolved from the staging projection.
struct UploadInfo {
    organisation: String,
    name: String,
}

#[derive(Clone)]
pub struct ComponentService {
    event_store: EventStore,
    db: PgPool,
    object_store: crate::object_store::ObjectStore,
}

impl ComponentService {
    pub fn new(
        event_store: EventStore,
        db: PgPool,
        object_store: crate::object_store::ObjectStore,
    ) -> Self {
        Self {
            event_store,
            db,
            object_store,
        }
    }

    // ----------------------------------------------------------
    // Commands
    // ----------------------------------------------------------

    /// Begin a component version upload. Returns the upload_id (UUID).
    ///
    /// Projections updated atomically:
    /// - `component_staging` row inserted (status='staged')
    pub async fn begin_upload(
        &self,
        organisation: &str,
        name: &str,
        version: &str,
    ) -> anyhow::Result<Uuid> {
        let key = component::stream_key(organisation, name);
        let mut root = self
            .event_store
            .load_or_default::<ComponentAggregate>(&key)
            .await?;

        let upload_id = ComponentAggregate::begin_upload(&mut root, organisation, name, version)?;

        let org = organisation.to_string();
        let name_owned = name.to_string();
        let version_owned = version.to_string();

        self.event_store
            .save_with(&mut root, move |_events, tx| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO component_staging (id, name, organisation, version, status)
                         VALUES ($1, $2, $3, $4, 'staged')
                         ON CONFLICT (name, organisation, version)
                         DO UPDATE SET id = $1, status = 'staged', updated = now()",
                    )
                    .bind(upload_id)
                    .bind(&name_owned)
                    .bind(&org)
                    .bind(&version_owned)
                    .execute(&mut **tx)
                    .await
                    .context("insert staging projection")?;
                    Ok(())
                })
            })
            .await?;

        Ok(upload_id)
    }

    /// Upload a file for an in-flight upload.
    ///
    /// Stores file content in S3, records metadata in DB via event store.
    pub async fn upload_file(
        &self,
        upload_id: Uuid,
        file_path: &str,
        file_content: &[u8],
    ) -> anyhow::Result<()> {
        let info = self.resolve_upload(upload_id).await?;
        let key = component::stream_key(&info.organisation, &info.name);

        let mut root = self
            .event_store
            .load_or_default::<ComponentAggregate>(&key)
            .await?;

        ComponentAggregate::upload_file(&mut root, upload_id, file_path)?;

        // Resolve version from staging
        let version: String =
            sqlx::query_scalar("SELECT version FROM component_staging WHERE id = $1")
                .bind(upload_id)
                .fetch_one(&self.db)
                .await
                .context("resolve version from staging")?;

        // Store file content in S3
        let s3_key = crate::object_store::keys::component_file(
            &info.organisation,
            &info.name,
            &version,
            file_path,
        );
        self.object_store
            .put(&s3_key, file_content)
            .await
            .context("store component file in S3")?;

        let file_path_owned = file_path.to_string();

        // Record metadata in DB (file_content is empty — content is in S3)
        let db_result = self
            .event_store
            .save_with(&mut root, move |_events, tx| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO component_files (component_id, file_path, file_content)
                         VALUES ($1, $2, ''::bytea)",
                    )
                    .bind(upload_id)
                    .bind(&file_path_owned)
                    .execute(&mut **tx)
                    .await
                    .context("insert component file metadata")?;
                    Ok(())
                })
            })
            .await;

        // Cleanup S3 if DB write failed
        if let Err(e) = &db_result {
            tracing::warn!("DB write failed after S3 upload, cleaning up S3 object: {e:#}");
            let _ = self.object_store.delete(&s3_key).await;
        }

        db_result?;
        Ok(())
    }

    /// Unpublish a previously-published version (TASKS/025).
    ///
    /// Atomically:
    /// - emits `VersionUnpublished` on the aggregate (audit trail);
    /// - deletes the version's row from the `components` projection so
    ///   subsequent `forest components show` / `forest global add` etc.
    ///   behave as if the version had never been published.
    ///
    /// The manifest blob and uploaded artifacts remain in object storage
    /// for now — a future GC sweep can reclaim them; the resolver no
    /// longer points at them after this method returns.
    ///
    /// Returns `Ok(true)` when a new event was recorded, `Ok(false)` on
    /// idempotent no-op (version was already Unpublished).
    pub async fn unpublish_version(
        &self,
        organisation: &str,
        name: &str,
        version: &str,
        actor: &str,
        reason: Option<&str>,
    ) -> anyhow::Result<bool> {
        let key = component::stream_key(organisation, name);
        let mut root = self
            .event_store
            .load_or_default::<ComponentAggregate>(&key)
            .await?;

        // Truncate reason at 1024 chars per the proto contract.
        let reason = reason.map(|r| {
            if r.len() > 1024 {
                r[..1024].to_string()
            } else {
                r.to_string()
            }
        });

        let recorded =
            ComponentAggregate::unpublish_version(&mut root, version, actor, reason.as_deref())?;

        if !recorded {
            // No-op: nothing changed on the aggregate, nothing to persist.
            return Ok(false);
        }

        let org = organisation.to_string();
        let name_owned = name.to_string();
        let version_owned = version.to_string();

        self.event_store
            .save_with(&mut root, move |_events, tx| {
                Box::pin(async move {
                    // Delete the projection row so read paths skip it.
                    // The aggregate retains the events for audit / replay.
                    sqlx::query(
                        "DELETE FROM components
                         WHERE organisation = $1 AND name = $2 AND version = $3",
                    )
                    .bind(&org)
                    .bind(&name_owned)
                    .bind(&version_owned)
                    .execute(&mut **tx)
                    .await
                    .context("delete unpublished component projection")?;
                    Ok(())
                })
            })
            .await?;

        Ok(true)
    }

    /// Explicitly abort an in-flight upload (TASKS/023).
    ///
    /// The CLI's `AbortOnDrop` guard calls this on early return / panic, so
    /// the next `begin_upload` for the same version doesn't have to rely on
    /// the supersede path. Idempotent — aborting an unknown / committed /
    /// already-aborted upload is a no-op success.
    ///
    /// Projections updated atomically:
    /// - `component_staging` status set to 'aborted' (best-effort row touch)
    pub async fn abort_upload(&self, upload_id: Uuid, reason: &str) -> anyhow::Result<()> {
        let info = match self.resolve_upload(upload_id).await {
            Ok(i) => i,
            // Unknown upload — already cleaned up or never existed. Idempotent.
            Err(_) => return Ok(()),
        };

        let key = component::stream_key(&info.organisation, &info.name);
        let mut root = self
            .event_store
            .load_or_default::<ComponentAggregate>(&key)
            .await?;

        // Cap reason at 256 chars; longer strings are truncated silently
        // (the proto comment promises this; tests cover the boundary).
        let reason = if reason.len() > 256 {
            &reason[..256]
        } else {
            reason
        };

        ComponentAggregate::abort_upload(&mut root, upload_id, reason)?;

        // If the aggregate command was a no-op (no event recorded), skip the
        // save_with path entirely — there's nothing to persist.
        if root.pending_count() == 0 {
            return Ok(());
        }

        self.event_store
            .save_with(&mut root, move |_events, tx| {
                Box::pin(async move {
                    sqlx::query(
                        "UPDATE component_staging SET status = 'aborted', updated = now()
                         WHERE id = $1 AND status = 'staged'",
                    )
                    .bind(upload_id)
                    .execute(&mut **tx)
                    .await
                    .context("mark staging row aborted")?;
                    Ok(())
                })
            })
            .await?;

        Ok(())
    }

    /// Commit (publish) an upload.
    ///
    /// Projections updated atomically:
    /// - `components` row upserted
    /// - `component_staging` status set to 'committed'
    ///
    /// TASKS/023: refuses to commit a binary-shaped upload when no manifest
    /// has been recorded against this upload's aggregate. This closes the
    /// ghost-version path where `_meta/describe` (or any other manifest
    /// build step) fails after `begin_upload` + binary upload but before
    /// `publish_manifest`. CUE-only / Deno publishes have no manifest
    /// validator today and are exempt.
    pub async fn commit_upload(&self, upload_id: Uuid) -> anyhow::Result<()> {
        let info = self.resolve_upload(upload_id).await?;
        let key = component::stream_key(&info.organisation, &info.name);

        // Pre-flight: did any binary artifact land for this upload? If so,
        // a manifest is mandatory before commit. Reading from the projection
        // outside the save_with closure is fine — artifacts are append-only
        // and the worst race (someone uploads an artifact between this read
        // and the commit) only relaxes the check in our favour.
        let has_binary_artifacts: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM component_artifacts WHERE component_id = $1)",
        )
        .bind(upload_id)
        .fetch_one(&self.db)
        .await
        .unwrap_or(false);

        let mut root = self
            .event_store
            .load_or_default::<ComponentAggregate>(&key)
            .await?;

        let version =
            ComponentAggregate::publish_version(&mut root, upload_id, has_binary_artifacts)?;

        let org = info.organisation;
        let name = info.name;

        // Keep copies for the OCI publish step (after the closure moves org/name)
        let org_for_oci = org.clone();
        let name_for_oci = name.clone();
        let version_for_oci = version.clone();

        self.event_store
            .save_with(&mut root, move |_events, tx| {
                Box::pin(async move {
                    // Detect kind:
                    //   - "binary": at least one binary artifact uploaded
                    //   - "external": manifest declared kind=external (no upload)
                    //   - "files": neither (v1 cue-only)
                    let has_artifacts: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM component_artifacts WHERE component_id = $1)",
                    )
                    .bind(upload_id)
                    .fetch_one(&mut **tx)
                    .await
                    .unwrap_or(false);

                    // The shape was computed and stashed by publish_manifest.
                    // Read it (may be NULL for legacy pre-spec manifests).
                    let staged_shape: Option<String> =
                        sqlx::query_scalar("SELECT shape FROM component_staging WHERE id = $1")
                            .bind(upload_id)
                            .fetch_one(&mut **tx)
                            .await
                            .unwrap_or(None);

                    let kind = match staged_shape.as_deref() {
                        Some("tool_external") => "external",
                        _ if has_artifacts => "binary",
                        _ => "files",
                    };
                    let shape = staged_shape.as_deref().unwrap_or("component");

                    sqlx::query(
                        "INSERT INTO components (id, name, organisation, version, kind, shape)
                         VALUES ($1, $2, $3, $4, $5, $6)
                         ON CONFLICT (name, organisation, version)
                         DO UPDATE SET kind = $5, shape = $6, updated = now()",
                    )
                    .bind(upload_id)
                    .bind(&name)
                    .bind(&org)
                    .bind(&version)
                    .bind(kind)
                    .bind(shape)
                    .execute(&mut **tx)
                    .await
                    .context("upsert component projection")?;

                    sqlx::query(
                        "UPDATE component_staging SET status = 'committed', updated = now()
                         WHERE id = $1 AND status = 'staged'",
                    )
                    .bind(upload_id)
                    .execute(&mut **tx)
                    .await
                    .context("update staging status")?;

                    Ok(())
                })
            })
            .await?;

        // Auto-publish CUE files as an OCI artifact for CUE module resolution.
        // This runs after commit so the component is in a consistent state.
        let cue_files = self.get_cue_files(upload_id).await?;
        if !cue_files.is_empty() {
            if let Err(e) = crate::oci_registry::publish_cue_module(
                &self.object_store,
                &org_for_oci,
                &name_for_oci,
                &version_for_oci,
                cue_files,
            )
            .await
            {
                tracing::warn!("failed to publish CUE module as OCI artifact: {e:#}");
                // Non-fatal — the component is committed, just the OCI artifact failed
            }
        }

        Ok(())
    }

    /// Get CUE files uploaded for a component (for OCI packaging).
    async fn get_cue_files(&self, component_id: Uuid) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
        let rows = sqlx::query(
            "SELECT file_path FROM component_files
             WHERE component_id = $1 AND file_path LIKE '%.cue'
             ORDER BY file_path",
        )
        .bind(component_id)
        .fetch_all(&self.db)
        .await?;

        let mut files = Vec::new();
        for row in rows {
            use sqlx::Row;
            let path: String = row.get("file_path");

            // Read from S3
            let comp = sqlx::query(
                "SELECT name, organisation, version FROM components WHERE id = $1
                 UNION ALL
                 SELECT name, organisation, version FROM component_staging WHERE id = $1
                 LIMIT 1",
            )
            .bind(component_id)
            .fetch_optional(&self.db)
            .await?;

            if let Some(comp) = comp {
                let org: String = comp.get("organisation");
                let name: String = comp.get("name");
                let ver: String = comp.get("version");
                let s3_key = crate::object_store::keys::component_file(&org, &name, &ver, &path);

                if let Ok(content) = self.object_store.get(&s3_key).await {
                    files.push((path, content));
                }
            }
        }

        Ok(files)
    }

    // ----------------------------------------------------------
    // Queries (read from projections)
    // ----------------------------------------------------------

    /// Get the latest version of a component.
    pub async fn get_component(
        &self,
        name: &str,
        organisation: &str,
    ) -> anyhow::Result<Option<ComponentVersion>> {
        // Ordering happens in Rust, not SQL. The previous
        // `split_part(version, '.', N)::int DESC` could not merely mis-sort a
        // semver prerelease — it made the whole query *fail*: for `0.1.7-ci.1`
        // the third dot-separated part is `7-ci`, and `'7-ci'::int` raises
        // `invalid input syntax for type integer`. One prerelease anywhere in a
        // component's history took out `forest components show` for that
        // component entirely (DATA-583).
        //
        // `semver::Version` also gets the precedence rules right, which the
        // numeric split never could: a prerelease sorts *below* its release, so
        // `0.1.8-rc.1 < 0.1.8`.
        let rows = sqlx::query(
            "SELECT id, name, organisation, version
             FROM components
             WHERE name = $1 AND organisation = $2",
        )
        .bind(name)
        .bind(organisation)
        .fetch_all(&self.db)
        .await
        .context("get component")?;

        // Latest *stable*, falling back to the highest of any kind when no
        // stable release exists. Same rule fungus applies to ports (DATA-499)
        // and the same one `list_org_tools` below already used — this brings
        // the component detail view in line with both, and with the CLI, where
        // a range spec no longer resolves to a prerelease.
        //
        // Ordering alone was not enough: a prerelease sorts below *its own*
        // release but still above the previous one, so `0.2.0-rc.1` outranked a
        // shipped `0.1.8` and presented itself as the component's latest
        // version. The fallback keeps a component that has only ever published
        // release candidates from reporting no version at all.
        let versions: Vec<String> = rows.iter().map(|r| r.get("version")).collect();
        let chosen = pick_latest_version(&versions);
        let latest =
            chosen.and_then(|want| rows.iter().find(|r| &r.get::<String, _>("version") == want));

        Ok(latest.map(|r| ComponentVersion {
            id: r.get::<Uuid, _>("id").to_string(),
            name: r.get("name"),
            organisation: r.get("organisation"),
            version: r.get("version"),
        }))
    }

    /// Get a specific component version.
    pub async fn get_component_version(
        &self,
        name: &str,
        organisation: &str,
        version: &str,
    ) -> anyhow::Result<Option<ComponentVersion>> {
        let row = sqlx::query(
            "SELECT id, name, organisation, version
             FROM components
             WHERE name = $1 AND organisation = $2 AND version = $3",
        )
        .bind(name)
        .bind(organisation)
        .bind(version)
        .fetch_optional(&self.db)
        .await
        .context("get component version")?;

        Ok(row.map(|r| ComponentVersion {
            id: r.get::<Uuid, _>("id").to_string(),
            name: r.get("name"),
            organisation: r.get("organisation"),
            version: r.get("version"),
        }))
    }

    /// Stream files for a published component.
    /// Fetches file paths from DB metadata, content from S3.
    pub async fn get_files(
        &self,
        component_id: Uuid,
        file_stream: FileStream,
    ) -> anyhow::Result<()> {
        // Get component identity for S3 key construction
        let comp = sqlx::query("SELECT name, organisation, version FROM components WHERE id = $1")
            .bind(component_id)
            .fetch_optional(&self.db)
            .await
            .context("get component for file streaming")?;

        let Some(comp) = comp else {
            file_stream.push_done().await?;
            return Ok(());
        };

        let org: String = comp.get("organisation");
        let name: String = comp.get("name");
        let version: String = comp.get("version");

        // Get file paths from metadata
        let rows = sqlx::query(
            "SELECT file_path FROM component_files
             WHERE component_id = $1
             ORDER BY file_path ASC",
        )
        .bind(component_id)
        .fetch_all(&self.db)
        .await
        .context("list component files")?;

        for row in rows {
            let path: String = row.get("file_path");
            let s3_key = crate::object_store::keys::component_file(&org, &name, &version, &path);

            match self.object_store.get(&s3_key).await {
                Ok(content) => {
                    if let Err(e) = file_stream.push_file(&path, &content).await {
                        file_stream.push_err(e).await?;
                        return Ok(());
                    }
                }
                Err(e) => {
                    tracing::warn!("failed to get file from S3: {path}: {e}");
                    file_stream.push_err(e).await?;
                    return Ok(());
                }
            }
        }

        file_stream.push_done().await?;
        Ok(())
    }

    // ----------------------------------------------------------
    // v2: Binary component methods
    // ----------------------------------------------------------

    /// Store a binary artifact for an in-flight upload.
    /// Binary content goes to S3; metadata recorded in `component_artifacts`.
    pub async fn upload_binary(
        &self,
        upload_id: Uuid,
        os: &str,
        arch: &str,
        sha256: &str,
        binary_content: &[u8],
    ) -> anyhow::Result<u64> {
        let info = self.resolve_upload(upload_id).await?;
        let size_bytes = binary_content.len() as i64;

        // Resolve version from staging
        let version: String =
            sqlx::query_scalar("SELECT version FROM component_staging WHERE id = $1")
                .bind(upload_id)
                .fetch_one(&self.db)
                .await
                .context("resolve version from staging")?;

        // Store binary in S3
        let s3_key = crate::object_store::keys::component_binary(
            &info.organisation,
            &info.name,
            &version,
            os,
            arch,
        );
        self.object_store
            .put(&s3_key, binary_content)
            .await
            .context("store binary in S3")?;

        tracing::info!(
            key = %s3_key,
            size = size_bytes,
            "stored component binary in S3"
        );

        // Record artifact metadata with storage_path pointing to S3
        let db_result = sqlx::query(
            "INSERT INTO component_artifacts (component_id, version, os, arch, sha256, size_bytes, storage_path)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (component_id, version, os, arch)
             DO UPDATE SET sha256 = $5, size_bytes = $6, storage_path = $7, created_at = now()",
        )
        .bind(upload_id)
        .bind(&version)
        .bind(os)
        .bind(arch)
        .bind(sha256)
        .bind(size_bytes)
        .bind(&s3_key)
        .execute(&self.db)
        .await
        .context("record artifact metadata");

        // Cleanup S3 if DB write failed
        if let Err(e) = &db_result {
            tracing::warn!("DB write failed after S3 upload, cleaning up S3 object: {e:#}");
            let _ = self.object_store.delete(&s3_key).await;
        }

        db_result?;
        Ok(size_bytes as u64)
    }

    /// Publish a manifest for an in-flight upload.
    ///
    /// Validates the manifest against TASKS/018-global-tools.md §1a.2 rules
    /// 1–7 via the shared `forest-manifest` parser. Derives the manifest's
    /// shape (§1a.2e) and forwards it to the caller for persistence on the
    /// upgrade path (the `components` row gets `shape` updated by
    /// `commit_upload`, which we leave alone here — `publish_manifest` only
    /// touches `component_manifests`).
    ///
    /// TASKS/023: also records a `ManifestRecorded` aggregate event so that
    /// `publish_version` can require manifest presence as a precondition.
    /// The event lands atomically with the projection writes via
    /// `EventStore::save_with`.
    pub async fn publish_manifest(
        &self,
        upload_id: Uuid,
        manifest_json: &str,
    ) -> anyhow::Result<()> {
        let info = self.resolve_upload(upload_id).await?;

        // §1a.2 rule 5: payload size cap (64 KiB) — defence against
        // malicious manifests; the parser is fast but the JSON could
        // still be pathologically nested.
        const MAX_MANIFEST_SIZE: usize = 64 * 1024;
        if manifest_json.len() > MAX_MANIFEST_SIZE {
            anyhow::bail!(
                "manifest exceeds maximum size of {MAX_MANIFEST_SIZE} bytes (got {})",
                manifest_json.len()
            );
        }

        // §1a.2 rules 1–4, 7: structural validation + shape derivation.
        let parsed = forest_manifest::parse(manifest_json)
            .map_err(|e| anyhow::anyhow!("invalid manifest: {e:?}"))?;
        let shape = shape_to_str(parsed.shape).to_string();

        let key = component::stream_key(&info.organisation, &info.name);
        let mut root = self
            .event_store
            .load_or_default::<ComponentAggregate>(&key)
            .await?;

        ComponentAggregate::record_manifest(&mut root, upload_id)?;

        let manifest_json = manifest_json.to_string();
        // Content identity for immutability (TASKS/024). Canonical hash of the
        // manifest; the manifest embeds binary shas, so this pins binaries too.
        // Stored alongside the manifest now; enforcement is wired separately.
        let manifest_hash = forest_manifest::hash::manifest_hash(&manifest_json)
            .map_err(|e| anyhow::anyhow!("hashing manifest: {e:?}"))?;

        self.event_store
            .save_with(&mut root, move |_events, tx| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO component_manifests (component_id, version, manifest_json, manifest_hash)
                         SELECT $1, cs.version, $2::jsonb, $3
                         FROM component_staging cs WHERE cs.id = $1
                         ON CONFLICT (component_id, version)
                         DO UPDATE SET manifest_json = $2::jsonb, manifest_hash = $3, created_at = now()",
                    )
                    .bind(upload_id)
                    .bind(&manifest_json)
                    .bind(&manifest_hash)
                    .execute(&mut **tx)
                    .await
                    .context("publish manifest")?;

                    // Stash the shape on `component_staging` so `commit_upload` can
                    // promote it onto the `components` row in the same transaction
                    // as the upload finalization.
                    sqlx::query(
                        "UPDATE component_staging SET shape = $1 WHERE id = $2",
                    )
                    .bind(&shape)
                    .bind(upload_id)
                    .execute(&mut **tx)
                    .await
                    .context("update staging shape")?;

                    Ok(())
                })
            })
            .await?;

        Ok(())
    }

    /// Get the manifest for a specific component version.
    pub async fn get_manifest(
        &self,
        organisation: &str,
        name: &str,
        version: &str,
    ) -> anyhow::Result<Option<String>> {
        let row = sqlx::query(
            "SELECT cm.manifest_json::text as manifest_json
             FROM component_manifests cm
             JOIN components c ON c.id = cm.component_id
             WHERE c.organisation = $1 AND c.name = $2 AND cm.version = $3",
        )
        .bind(organisation)
        .bind(name)
        .bind(version)
        .fetch_optional(&self.db)
        .await
        .context("get manifest")?;

        Ok(row.map(|r| r.get("manifest_json")))
    }

    /// List all versions of a component with platform info.
    pub async fn list_versions(
        &self,
        organisation: &str,
        name: &str,
    ) -> anyhow::Result<Vec<ComponentVersionInfo>> {
        let rows = sqlx::query(
            "SELECT c.version, c.kind, c.created,
                    COALESCE(
                        (SELECT json_agg(ca.os || '_' || ca.arch)
                         FROM component_artifacts ca
                         WHERE ca.component_id = c.id AND ca.version = c.version),
                        '[]'::json
                    )::text as platforms,
                    COALESCE(
                        (SELECT cm.manifest_json->>'protocol_version'
                         FROM component_manifests cm
                         WHERE cm.component_id = c.id AND cm.version = c.version),
                        ''
                    ) as protocol_version
             FROM components c
             WHERE c.organisation = $1 AND c.name = $2",
        )
        .bind(organisation)
        .bind(name)
        .fetch_all(&self.db)
        .await
        .context("list versions")?;

        // Sorted in Rust for the same reason as `get_component` above: the
        // `::int` cast on a dot-part raised a Postgres error on any semver
        // prerelease rather than just ordering it oddly. Newest first.
        let mut rows = rows;
        rows.sort_by(|a, b| {
            let (av, bv): (String, String) = (a.get("version"), b.get("version"));
            compare_versions(&bv, &av)
        });

        let mut versions = Vec::new();
        for row in rows {
            let platforms_json: String = row.get("platforms");
            let platforms: Vec<String> = serde_json::from_str(&platforms_json).unwrap_or_default();

            versions.push(ComponentVersionInfo {
                version: row.get("version"),
                protocol_version: row.get("protocol_version"),
                kind: row.get("kind"),
                platforms,
                created_at: row
                    .get::<chrono::DateTime<chrono::Utc>, _>("created")
                    .to_rfc3339(),
            });
        }

        Ok(versions)
    }

    /// List the tool-y components published under `organisation`.
    ///
    /// Filters to `shape IN ('hybrid_component', 'tool_binary', 'tool_external')`
    /// per §1a.2c, picks the highest non-prerelease semver per (org, name),
    /// reads the tool facet from the latest manifest, and (for externals)
    /// the upstream host from `platforms[*].url`.
    pub async fn list_org_tools(&self, organisation: &str) -> anyhow::Result<Vec<OrgToolRow>> {
        let rows = sqlx::query(
            "SELECT c.organisation, c.name, c.version, c.shape,
                    cm.manifest_json::text AS manifest_json
             FROM components c
             LEFT JOIN component_manifests cm
               ON cm.component_id = c.id AND cm.version = c.version
             WHERE c.organisation = $1
               AND c.shape IN ('hybrid_component', 'tool_binary', 'tool_external')",
        )
        .bind(organisation)
        .fetch_all(&self.db)
        .await
        .context("list org tools query")?;

        // Group by name, pick highest non-prerelease semver.
        let mut by_name: std::collections::BTreeMap<String, (String, String, Option<String>)> =
            std::collections::BTreeMap::new();
        for r in rows {
            let name: String = r.get("name");
            let version: String = r.get("version");
            let shape: String = r.get("shape");
            let manifest_json: Option<String> = r.get("manifest_json");

            if is_prerelease(&version) {
                continue;
            }

            let candidate = (version.clone(), shape, manifest_json);
            match by_name.get(&name) {
                None => {
                    by_name.insert(name, candidate);
                }
                Some((existing_version, _, _)) => {
                    if version_gt(&candidate.0, existing_version) {
                        by_name.insert(name, candidate);
                    }
                }
            }
        }

        let mut out = Vec::with_capacity(by_name.len());
        for (name, (version, shape, manifest_json)) in by_name {
            let manifest_value: Option<serde_json::Value> = manifest_json
                .as_deref()
                .and_then(|j| serde_json::from_str(j).ok());

            // Tool facet
            let tool = manifest_value
                .as_ref()
                .and_then(|v| v.get("tool"))
                .map(|t| ToolFacetRow {
                    name: t
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    argv_passthrough: t
                        .get("argv_passthrough")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true),
                    description: t
                        .get("description")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                });

            // Upstream host: only meaningful for tool_external.
            let upstream_host = if shape == "tool_external" {
                manifest_value
                    .as_ref()
                    .and_then(|v| v.get("platforms"))
                    .and_then(|p| p.as_object())
                    .and_then(|m| m.values().next())
                    .and_then(|p| p.get("url"))
                    .and_then(|u| u.as_str())
                    .and_then(extract_host)
            } else {
                None
            };

            out.push(OrgToolRow {
                organisation: organisation.to_string(),
                name,
                latest_version: version,
                shape,
                tool,
                upstream_host,
            });
        }
        // Stable order: by name.
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// Download a binary artifact from S3 (returns raw bytes).
    pub async fn download_binary(
        &self,
        organisation: &str,
        name: &str,
        version: &str,
        os: &str,
        arch: &str,
    ) -> anyhow::Result<Vec<u8>> {
        let s3_key = self
            .binary_storage_path(organisation, name, version, os, arch)
            .await?;

        self.object_store.get(&s3_key).await.with_context(|| {
            format!("download binary from S3: {organisation}/{name}@{version} ({os}/{arch})")
        })
    }

    /// Stream a binary artifact from S3 without buffering it (DATA-505).
    ///
    /// Preferred over [`Self::download_binary`] for the download RPC: that one
    /// waits for the entire object before the handler can send anything, so
    /// time-to-first-byte is the whole S3 GET and peak memory is the artifact
    /// size (twice over, once the handler chunks it). This lets the handler
    /// forward chunks as they arrive.
    pub async fn download_binary_stream(
        &self,
        organisation: &str,
        name: &str,
        version: &str,
        os: &str,
        arch: &str,
    ) -> anyhow::Result<crate::object_store::ByteStream> {
        let s3_key = self
            .binary_storage_path(organisation, name, version, os, arch)
            .await?;

        self.object_store
            .get_stream(&s3_key)
            .await
            .with_context(|| {
                format!("stream binary from S3: {organisation}/{name}@{version} ({os}/{arch})")
            })
    }

    /// Resolve the S3 key for a published platform binary.
    async fn binary_storage_path(
        &self,
        organisation: &str,
        name: &str,
        version: &str,
        os: &str,
        arch: &str,
    ) -> anyhow::Result<String> {
        // Look up storage_path from component_artifacts
        let storage_path: Option<String> = sqlx::query_scalar(
            "SELECT ca.storage_path
             FROM component_artifacts ca
             JOIN components c ON c.id = ca.component_id
             WHERE c.organisation = $1 AND c.name = $2 AND ca.version = $3
               AND ca.os = $4 AND ca.arch = $5",
        )
        .bind(organisation)
        .bind(name)
        .bind(version)
        .bind(os)
        .bind(arch)
        .fetch_optional(&self.db)
        .await
        .context("look up binary artifact")?
        .flatten();

        storage_path.with_context(|| {
            format!("binary not found: {organisation}/{name}@{version} ({os}/{arch})")
        })
    }

    // ----------------------------------------------------------
    // Registry UI / search
    // ----------------------------------------------------------

    /// Search components with visibility filtering.
    /// - `see_all`: service accounts bypass visibility (true = no filtering)
    /// - `member_orgs`: org names the caller belongs to (empty for anonymous)
    /// Results include: public project components + private components from member_orgs.
    pub async fn search_components(
        &self,
        query: &str,
        organisation_filter: &str,
        limit: i64,
        offset: i64,
        see_all: bool,
        member_orgs: &[String],
    ) -> anyhow::Result<(Vec<forest_grpc_interface::ComponentSummary>, i32)> {
        let is_search = !query.is_empty();
        let is_org_filter = !organisation_filter.is_empty();
        let search_pattern = format!("%{query}%");

        // Visibility filter: when not see_all, show public projects + member org components.
        // $7 = see_all (skip filter), $8 = member_orgs array
        let rows = sqlx::query(
            "SELECT c.organisation, c.name, c.version, c.kind, c.shape, c.created, c.updated,
                    COALESCE((SELECT cm.manifest_json::text FROM component_manifests cm WHERE cm.component_id = c.id LIMIT 1), '') as manifest_json,
                    (SELECT count(*) FROM components c2 WHERE c2.organisation = c.organisation AND c2.name = c.name) as version_count,
                    COALESCE((SELECT p.visibility FROM projects p WHERE p.organisation = c.organisation AND p.project = c.name LIMIT 1), 'private') as visibility
             FROM components c
             WHERE ($1 = false OR c.name ILIKE $2 OR c.organisation ILIKE $2)
               AND ($3 = false OR c.organisation = $4)
               AND ($7 = true OR c.organisation = ANY($8) OR EXISTS (
                   SELECT 1 FROM projects p
                   WHERE p.organisation = c.organisation AND p.project = c.name
                     AND p.visibility = 'public'
               ))
             ORDER BY c.updated DESC
             LIMIT $5 OFFSET $6",
        )
        .bind(is_search)
        .bind(&search_pattern)
        .bind(is_org_filter)
        .bind(organisation_filter)
        .bind(limit)
        .bind(offset)
        .bind(see_all)
        .bind(member_orgs)
        .fetch_all(&self.db)
        .await
        .context("search components")?;

        let total: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM components c
             WHERE ($1 = false OR c.name ILIKE $2 OR c.organisation ILIKE $2)
               AND ($3 = false OR c.organisation = $4)
               AND ($5 = true OR c.organisation = ANY($6) OR EXISTS (
                   SELECT 1 FROM projects p
                   WHERE p.organisation = c.organisation AND p.project = c.name
                     AND p.visibility = 'public'
               ))",
        )
        .bind(is_search)
        .bind(&search_pattern)
        .bind(is_org_filter)
        .bind(organisation_filter)
        .bind(see_all)
        .bind(member_orgs)
        .fetch_one(&self.db)
        .await
        .unwrap_or(0);

        let summaries = rows
            .into_iter()
            .map(|r| {
                use sqlx::Row;
                let shape_str: String = r.get("shape");
                let manifest_json: String = r.get("manifest_json");
                let manifest_value: Option<serde_json::Value> =
                    serde_json::from_str(&manifest_json).ok();
                let description = manifest_value
                    .as_ref()
                    .and_then(|v| v.get("description"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let tool = manifest_value
                    .as_ref()
                    .and_then(|v| v.get("tool"))
                    .map(|t| forest_grpc_interface::ToolFacet {
                        name: t
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        argv_passthrough: t
                            .get("argv_passthrough")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(true),
                        description: t
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    });
                let methods: Vec<String> = manifest_value
                    .as_ref()
                    .and_then(|v| v.get("methods"))
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|m| m.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                let upstream_host = if shape_str == "tool_external" {
                    manifest_value
                        .as_ref()
                        .and_then(|v| v.get("platforms"))
                        .and_then(|p| p.as_object())
                        .and_then(|m| m.values().next())
                        .and_then(|p| p.get("url"))
                        .and_then(|u| u.as_str())
                        .and_then(extract_host)
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                forest_grpc_interface::ComponentSummary {
                    organisation: r.get("organisation"),
                    name: r.get("name"),
                    latest_version: r.get("version"),
                    kind: r.get("kind"),
                    description,
                    created_at: r
                        .get::<chrono::DateTime<chrono::Utc>, _>("created")
                        .to_rfc3339(),
                    updated_at: r
                        .get::<chrono::DateTime<chrono::Utc>, _>("updated")
                        .to_rfc3339(),
                    version_count: r.get::<i64, _>("version_count") as i32,
                    contracts: vec![],
                    visibility: r.get("visibility"),
                    shape: shape_str_to_proto(&shape_str) as i32,
                    tool,
                    methods,
                    upstream_host,
                }
            })
            .collect();

        Ok((summaries, total as i32))
    }

    /// Get full component detail for the registry UI.
    pub async fn get_component_detail(
        &self,
        organisation: &str,
        name: &str,
    ) -> anyhow::Result<Option<forest_grpc_interface::GetComponentDetailResponse>> {
        let latest = self.get_component(name, organisation).await?;
        let Some(latest) = latest else {
            return Ok(None);
        };

        let versions = self.list_versions(organisation, name).await?;
        let manifest = self
            .get_manifest(organisation, name, &latest.version)
            .await?;

        let visibility: String = sqlx::query_scalar::<_, String>(
            "SELECT COALESCE((SELECT p.visibility FROM projects p WHERE p.organisation = $1 AND p.project = $2 LIMIT 1), 'private')",
        )
        .bind(organisation)
        .bind(name)
        .fetch_one(&self.db)
        .await
        .unwrap_or_else(|_| "private".into());

        let manifest_value: Option<serde_json::Value> = manifest
            .as_ref()
            .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok());
        // Shape and kind are per-version (a component can be republished
        // with a different shape — e.g. tool_external → tool_binary). The
        // summary header must reflect the *latest* version, not whichever
        // row Postgres returns first.
        let shape_str: String = sqlx::query_scalar::<_, String>(
            "SELECT shape FROM components
             WHERE organisation = $1 AND name = $2 AND version = $3",
        )
        .bind(organisation)
        .bind(name)
        .bind(&latest.version)
        .fetch_one(&self.db)
        .await
        .unwrap_or_else(|_| "component".into());
        let kind_str: String = sqlx::query_scalar::<_, String>(
            "SELECT kind FROM components
             WHERE organisation = $1 AND name = $2 AND version = $3",
        )
        .bind(organisation)
        .bind(name)
        .bind(&latest.version)
        .fetch_one(&self.db)
        .await
        .unwrap_or_else(|_| "binary".into());
        let tool = manifest_value
            .as_ref()
            .and_then(|v| v.get("tool"))
            .map(|t| forest_grpc_interface::ToolFacet {
                name: t
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                argv_passthrough: t
                    .get("argv_passthrough")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true),
                description: t
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            });
        let methods: Vec<String> = manifest_value
            .as_ref()
            .and_then(|v| v.get("methods"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let upstream_host = if shape_str == "tool_external" {
            manifest_value
                .as_ref()
                .and_then(|v| v.get("platforms"))
                .and_then(|p| p.as_object())
                .and_then(|m| m.values().next())
                .and_then(|p| p.get("url"))
                .and_then(|u| u.as_str())
                .and_then(extract_host)
                .unwrap_or_default()
        } else {
            String::new()
        };

        let summary = forest_grpc_interface::ComponentSummary {
            organisation: organisation.into(),
            name: name.into(),
            latest_version: latest.version.clone(),
            kind: kind_str,
            description: manifest_value
                .as_ref()
                .and_then(|v| v.get("description"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            created_at: String::new(),
            updated_at: String::new(),
            version_count: versions.len() as i32,
            contracts: vec![],
            visibility,
            shape: shape_str_to_proto(&shape_str) as i32,
            tool,
            methods,
            upstream_host,
        };

        let version_infos = versions
            .into_iter()
            .map(|v| forest_grpc_interface::ComponentVersionInfo {
                version: v.version,
                protocol_version: v.protocol_version,
                kind: v.kind,
                platforms: v.platforms,
                created_at: v.created_at,
            })
            .collect();

        // Load README.md (if uploaded) from S3 at the latest version. The
        // file is stored alongside the binary via `upload_file` —
        // component_files records the path, S3 holds the bytes. Common
        // README filenames (`README.md`, `readme.md`) both work.
        let readme = load_component_readme(
            &self.db,
            &self.object_store,
            organisation,
            name,
            &latest.version,
        )
        .await
        .unwrap_or_default();

        Ok(Some(forest_grpc_interface::GetComponentDetailResponse {
            summary: Some(summary),
            versions: version_infos,
            readme,
            manifest_json: manifest.unwrap_or_default(),
            owners: vec![],
        }))
    }

    // ----------------------------------------------------------
    // Internal helpers
    // ----------------------------------------------------------

    /// Resolve upload_id → (organisation, name) from staging projection.
    async fn resolve_upload(&self, upload_id: Uuid) -> anyhow::Result<UploadInfo> {
        let row = sqlx::query(
            "SELECT organisation, name FROM component_staging
             WHERE id = $1 AND status = 'staged'",
        )
        .bind(upload_id)
        .fetch_optional(&self.db)
        .await
        .context("resolve upload")?
        .with_context(|| format!("upload {} not found or already committed", upload_id))?;

        Ok(UploadInfo {
            organisation: row.get("organisation"),
            name: row.get("name"),
        })
    }
}

// ============================================================
// FileStream — gRPC streaming helper
// ============================================================

pub struct FileStream {
    rx: Option<
        futures::channel::mpsc::Receiver<
            std::result::Result<forest_grpc_interface::GetComponentFilesResponse, tonic::Status>,
        >,
    >,
    tx: futures::channel::mpsc::Sender<
        std::result::Result<forest_grpc_interface::GetComponentFilesResponse, tonic::Status>,
    >,
}

impl Default for FileStream {
    fn default() -> Self {
        Self::new()
    }
}

impl FileStream {
    pub fn new() -> Self {
        let (tx, rx) = futures::channel::mpsc::channel(10);
        Self { tx, rx: Some(rx) }
    }

    pub fn take_stream(
        &mut self,
    ) -> Pin<
        Box<
            dyn Stream<
                    Item = std::result::Result<
                        forest_grpc_interface::GetComponentFilesResponse,
                        tonic::Status,
                    >,
                > + Send,
        >,
    > {
        Box::pin(self.rx.take().expect("to only take stream once"))
    }

    pub async fn push_err(&self, error: anyhow::Error) -> anyhow::Result<()> {
        self.tx
            .clone()
            .send(Err(tonic::Status::internal(error.to_string())))
            .await?;
        Ok(())
    }

    pub async fn push_file(&self, file_path: &str, file_content: &[u8]) -> anyhow::Result<()> {
        self.tx
            .clone()
            .send(Ok(forest_grpc_interface::GetComponentFilesResponse {
                msg: Some(
                    forest_grpc_interface::get_component_files_response::Msg::ComponentFile(
                        forest_grpc_interface::ComponentFile {
                            file_path: file_path.into(),
                            file_content: file_content.into(),
                        },
                    ),
                ),
            }))
            .await?;
        Ok(())
    }

    pub async fn push_done(mut self) -> anyhow::Result<()> {
        self.tx
            .send(Ok(forest_grpc_interface::GetComponentFilesResponse {
                msg: Some(
                    forest_grpc_interface::get_component_files_response::Msg::Done(
                        forest_grpc_interface::Done {},
                    ),
                ),
            }))
            .await?;
        self.tx.close_channel();
        Ok(())
    }
}

// ============================================================
// State integration
// ============================================================

pub trait ComponentServiceState {
    fn component_service(&self) -> ComponentService;
}

impl ComponentServiceState for crate::state::State {
    fn component_service(&self) -> ComponentService {
        ComponentService::new(
            self.event_store.clone(),
            self.db.clone(),
            self.object_store.clone(),
        )
    }
}

/// DATA-583 — semver ordering for the registry's "latest version" and the
/// version list.
///
/// These are pure ordering tests against `compare_versions`; the behaviour they
/// protect used to live in SQL, where it could not be tested at all without a
/// live database — which is part of why a prerelease took `components show`
/// down unnoticed.
#[cfg(test)]
mod version_ordering_tests {
    use super::compare_versions;
    use std::cmp::Ordering;

    /// Newest-first sort, matching what `list_versions` does.
    fn sorted_desc(versions: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = versions.iter().map(|s| s.to_string()).collect();
        v.sort_by(|a, b| compare_versions(b, a));
        v
    }

    #[test]
    fn a_prerelease_does_not_break_ordering() {
        // The regression. `0.1.7-ci.1` has `7-ci` as its third dot-part, which
        // the old `::int` cast could not parse — the query errored outright
        // rather than returning a wrong order.
        assert_eq!(
            sorted_desc(&["0.1.7", "0.1.8", "0.1.7-ci.1", "0.1.6"]),
            vec!["0.1.8", "0.1.7", "0.1.7-ci.1", "0.1.6"],
        );
    }

    #[test]
    fn a_prerelease_sorts_below_its_release() {
        // Semver precedence, and the property that keeps a release candidate
        // from advertising itself as the latest version.
        assert_eq!(compare_versions("0.1.8-rc.1", "0.1.8"), Ordering::Less);
        assert_eq!(
            compare_versions("1.0.0-alpha", "1.0.0-beta"),
            Ordering::Less
        );
    }

    #[test]
    fn a_prerelease_still_outranks_the_previous_release() {
        // 0.1.8-rc.1 is newer than 0.1.7 — which is exactly why a *high*
        // prerelease would become "latest" and why test tags are numbered
        // below the current release.
        assert_eq!(compare_versions("0.1.8-rc.1", "0.1.7"), Ordering::Greater);
    }

    #[test]
    fn double_digit_segments_compare_numerically_not_lexically() {
        // The one thing the old integer cast did get right, kept: a string
        // sort would put 0.1.9 above 0.1.10.
        assert_eq!(sorted_desc(&["0.1.9", "0.1.10"]), vec!["0.1.10", "0.1.9"]);
        assert_eq!(sorted_desc(&["0.9.0", "0.10.0"]), vec!["0.10.0", "0.9.0"]);
    }

    /// DATA-583 — the detail view reports the latest *stable* version.
    ///
    /// Ordering alone never fixed this: a prerelease sorts below its own
    /// release but still above the previous one, so an rc outranked a shipped
    /// release and advertised itself as latest.
    #[test]
    fn latest_version_prefers_a_stable_release_over_a_higher_prerelease() {
        let versions = vec![
            "0.1.7".to_string(),
            "0.1.8".to_string(),
            "0.2.0-rc.1".to_string(),
        ];
        assert_eq!(super::pick_latest_version(&versions).unwrap(), "0.1.8");
    }

    #[test]
    fn latest_version_falls_back_when_only_prereleases_exist() {
        // A component that has never cut a stable release must still report
        // something rather than nothing.
        let versions = vec!["0.1.0-rc.1".to_string(), "0.1.0-rc.2".to_string()];
        assert_eq!(super::pick_latest_version(&versions).unwrap(), "0.1.0-rc.2");
    }

    #[test]
    fn latest_version_is_the_highest_stable_not_merely_the_first() {
        let versions = vec![
            "0.1.10".to_string(),
            "0.1.9".to_string(),
            "0.1.2".to_string(),
        ];
        assert_eq!(super::pick_latest_version(&versions).unwrap(), "0.1.10");
    }

    #[test]
    fn latest_version_of_nothing_is_nothing() {
        assert!(super::pick_latest_version(&[]).is_none());
    }

    #[test]
    fn build_metadata_breaks_ties_deterministically() {
        // The semver *spec* says build metadata does not affect precedence,
        // which makes precedence only a partial order. The `semver` crate's
        // `Ord` deliberately deviates to give a total order, ranking build
        // metadata above its absence.
        //
        // A total order is what sorting a version list actually requires — a
        // partial one would let equal-precedence rows come back in arbitrary,
        // run-to-run-varying positions. Asserted here so the deviation is a
        // recorded decision rather than a surprise; no component in the
        // registry publishes build metadata today.
        assert_eq!(
            compare_versions("1.0.0+build.1", "1.0.0"),
            Ordering::Greater
        );
        assert_eq!(compare_versions("1.0.0", "1.0.0"), Ordering::Equal);
    }

    #[test]
    fn unparseable_versions_sort_below_and_never_panic() {
        // Old rows may hold values semver cannot parse. They must not take the
        // query out a second time, and must not outrank a real version.
        assert_eq!(compare_versions("not-a-version", "0.1.0"), Ordering::Less);
        assert_eq!(
            compare_versions("0.1.0", "not-a-version"),
            Ordering::Greater
        );
        // Total and stable among themselves.
        assert_eq!(compare_versions("latest", "latest"), Ordering::Equal);
        assert_eq!(
            sorted_desc(&["0.1.0", "latest", "0.2.0"]),
            vec!["0.2.0", "0.1.0", "latest"],
        );
    }

    #[test]
    fn the_latest_of_a_realistic_history_is_the_highest_release() {
        // pgjump's actual version history at the time of the bug.
        let history = ["0.1.5", "0.1.6", "0.1.7", "0.1.7-ci.1", "0.1.8"];
        let latest = history
            .iter()
            .copied()
            .max_by(|a, b| compare_versions(a, b))
            .unwrap();
        assert_eq!(latest, "0.1.8");
    }
}
