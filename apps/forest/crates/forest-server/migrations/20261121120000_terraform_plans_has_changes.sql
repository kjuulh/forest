-- Whether a saved plan changes anything (`terraform plan -detailed-exitcode`
-- exit 2). The plan guard ignores a plan that changes nothing: applying it
-- cannot move what another release plans against. NULL is a plan saved before
-- this column existed, and counts as one that changes something.

ALTER TABLE terraform_plans ADD COLUMN has_changes BOOLEAN;
