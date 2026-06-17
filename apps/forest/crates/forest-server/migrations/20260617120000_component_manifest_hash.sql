-- TASKS/024 (DATA-308): content identity for published versions.
--
-- Stores the canonical manifest hash (forest-manifest `hash::manifest_hash`,
-- computed in Rust at publish time) so the publish path can enforce version
-- immutability. Nullable: rows published before this column exists keep NULL
-- and are treated as "unknown hash → not re-publishable" by enforcement, which
-- matches today's behaviour (begin_upload already rejects re-publishing a
-- published version). No backfill needed. No behaviour change on its own —
-- enforcement is wired separately.
ALTER TABLE component_manifests ADD COLUMN IF NOT EXISTS manifest_hash TEXT;
