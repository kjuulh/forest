-- Which destination a legacy terraform state belongs to (understory-io/forest#285).
--
-- State ids used to be `<environment>.<project>` with no destination in them,
-- so two terraform destinations in one environment silently shared — and
-- fought over — a single state. New states are now keyed per destination.
-- States that already exist keep their id, so nothing live moves; this table
-- records which destination first used each such id, and a second destination
-- asking for it is refused instead of sharing it.
--
-- Purely additive: rows are written on first use by the server, nothing is
-- backfilled, and older server versions simply ignore the table.

CREATE TABLE terraform_state_bindings (
    state_id     TEXT        PRIMARY KEY,
    organisation TEXT        NOT NULL,
    destination  TEXT        NOT NULL,
    bound_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
