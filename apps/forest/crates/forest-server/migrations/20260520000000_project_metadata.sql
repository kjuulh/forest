-- specs/features/009-project-metadata.md
--
-- Adds project-level `description` (parallel to `readme`) and a blessed
-- `metadata` JSONB blob for the About-sidebar fields (git_url, homepage,
-- docs_url, support_url, domain, owner). Length caps enforced at the
-- gRPC boundary, not by column checks — JSONB stays permissive so we
-- can add more blessed keys later without a per-key migration.

ALTER TABLE projects
    ADD COLUMN description TEXT NOT NULL DEFAULT '',
    ADD COLUMN metadata JSONB NOT NULL DEFAULT '{}'::jsonb;
