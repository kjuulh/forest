-- Saved plans for the `forest/terraform@1` destination (forest#284).
--
-- A plan stage used to run `terraform plan` and throw the plan away; the deploy
-- stage then ran `terraform apply -auto-approve` from scratch, so the change a
-- reviewer approved was not the change that got applied. Now the plan stage
-- keeps the binary plan (`plan -out`), and the deploy stage of the same release
-- intent applies exactly that file — which terraform refuses to do if the state
-- moved in between.
--
-- One row per (intent, destination, work dir): a destination can render more
-- than one terraform root. The lock file travels with the plan so the apply
-- initialises the providers the plan was made with.
--
-- A plan file can contain every secret the state does (resolved data sources,
-- sensitive attribute values), so these rows are treated like state: nothing
-- reads them but the runner, and a row is deleted once its apply has been
-- attempted. Plans never applied (a rejected stage) are pruned after 7 days
-- whenever a new plan is saved.

CREATE TABLE terraform_plans (
    id                BIGSERIAL   PRIMARY KEY,
    release_intent_id UUID        NOT NULL,
    destination_id    UUID        NOT NULL,
    work_dir          TEXT        NOT NULL,
    plan              BYTEA       NOT NULL,
    lock_file         TEXT,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),

    UNIQUE (release_intent_id, destination_id, work_dir)
);

CREATE INDEX terraform_plans_created_at_idx ON terraform_plans (created_at);
