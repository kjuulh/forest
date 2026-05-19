-- specs/features/008-project-canonical.md §"Data-model: project README"
--
-- Decouples the project-level README from any single component's
-- `component_files`. Lets `forest project update --readme` change docs
-- without bumping a component version, and gives the project Overview
-- page a single source of truth for the markdown it renders.
--
-- 64 KiB is enforced at the gRPC boundary, not by a column check.

ALTER TABLE projects
    ADD COLUMN readme TEXT NOT NULL DEFAULT '';
