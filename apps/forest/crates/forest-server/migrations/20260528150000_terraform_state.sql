-- Durable terraform state for the `forest/terraform@1` destination type.
-- Replaces the in-memory BTreeMap that was wiped on every forest-server
-- restart.
--
-- State is append-only: each successful tofu apply POSTs a new row.
-- `get()` returns the latest version per `state_id`. Older rows stay
-- around so we can audit and (eventually) roll back. A cleanup job can
-- prune by `state_id` and `created_at` once we have a retention policy.

CREATE TABLE terraform_states (
    id         BIGSERIAL   PRIMARY KEY,
    state_id   TEXT        NOT NULL,
    content    TEXT        NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX terraform_states_state_id_id_idx
    ON terraform_states (state_id, id DESC);

-- Exclusive per-`state_id` lock terraform holds for the duration of an
-- apply. Survives forest-server restarts. If a runner dies mid-apply
-- the lock will outlive it and require an admin DELETE to clear (same
-- behaviour as `tofu force-unlock` against any HTTP backend).

CREATE TABLE terraform_state_locks (
    state_id    TEXT        PRIMARY KEY,
    lock_id     TEXT        NOT NULL,
    acquired_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
