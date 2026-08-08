CREATE TABLE workflow_run_verifier_effect_publications (
    run_id TEXT NOT NULL REFERENCES workflow_runs(run_id) ON DELETE CASCADE,
    verifier_run_id TEXT NOT NULL
        REFERENCES workflow_run_step_verifiers(verifier_run_id)
        ON DELETE CASCADE,
    effect_key TEXT NOT NULL CHECK(LENGTH(TRIM(effect_key)) > 0),
    generation INTEGER NOT NULL CHECK(generation >= 1),
    owner_id TEXT NOT NULL CHECK(LENGTH(TRIM(owner_id)) > 0),
    owner_instance_id TEXT NOT NULL CHECK(LENGTH(TRIM(owner_instance_id)) > 0),
    published_at_ms INTEGER NOT NULL,
    PRIMARY KEY(verifier_run_id, effect_key)
);

CREATE INDEX idx_workflow_run_verifier_effect_publications_generation
    ON workflow_run_verifier_effect_publications(run_id, generation, published_at_ms);
