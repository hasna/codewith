ALTER TABLE background_agent_runs
ADD COLUMN model_attestation_json TEXT CHECK(
    model_attestation_json IS NULL OR json_valid(model_attestation_json)
);
