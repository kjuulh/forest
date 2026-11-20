ALTER TABLE terraform_state_locks
    ADD COLUMN operation  TEXT NOT NULL DEFAULT '',
    ADD COLUMN who        TEXT NOT NULL DEFAULT '',
    ADD COLUMN info       TEXT NOT NULL DEFAULT '',
    ADD COLUMN tf_version TEXT NOT NULL DEFAULT '',
    ADD COLUMN tf_created TEXT NOT NULL DEFAULT '',
    ADD COLUMN path       TEXT NOT NULL DEFAULT '',
    ADD COLUMN principal  TEXT,
    ADD COLUMN release_id UUID,
    ADD COLUMN replica    TEXT;

CREATE INDEX terraform_state_locks_release_id_idx
    ON terraform_state_locks (release_id)
    WHERE release_id IS NOT NULL;
