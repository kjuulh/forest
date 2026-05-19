-- Make destination names unique per-organisation instead of globally.
--
-- Previously destinations.name had a global UNIQUE index, which meant two
-- orgs could not pick the same destination name. That made every name
-- lookup safely return at most one row, but it also leaked org boundaries
-- semantically (orgs shouldn't have to coordinate on names).
--
-- Switch the constraint to (organisation, name). All callers must now
-- supply both fields when identifying a destination by name.

DROP INDEX IF EXISTS idx_destinations_name;
CREATE UNIQUE INDEX idx_destinations_org_name ON destinations (organisation, name);
