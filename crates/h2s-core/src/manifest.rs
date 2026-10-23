//! Reproducibility manifests: everything needed to re-run or audit an experiment.

use h2s_api::{Isolation, KeyValue, Manifest, ManifestJob, SweepIdentity};

/// Manifest format version.
pub const SCHEMA_VERSION: u32 = 1;

/// Inputs gathered by the server.
#[derive(Debug, Clone)]
pub struct ManifestInput {
    pub generated_at: String,
    pub studio_version: String,
    pub experiment: String,
    pub harness2_commit: Option<String>,
    pub dependency_pins: Vec<KeyValue>,
    pub models: Vec<KeyValue>,
    pub env_names: Vec<String>,
    pub isolation: Isolation,
    pub network_policy: String,
    pub jobs: Vec<ManifestJob>,
    pub sweep_identities: Vec<SweepIdentity>,
    pub results_digest: String,
    pub jobbench_revision: Option<String>,
}

/// Assemble the manifest (sorting lists so equal inputs give byte-equal JSON).
#[must_use]
pub fn build(input: ManifestInput) -> Manifest {
    Manifest {
        schema_version: SCHEMA_VERSION,
        generated_at: input.generated_at,
        studio_version: input.studio_version,
        experiment: input.experiment,
        harness2_commit: input.harness2_commit,
        dependency_pins: input.dependency_pins,
        models: input.models,
        env_names: input.env_names,
        isolation: input.isolation,
        network_policy: input.network_policy,
        jobs: input.jobs,
        sweep_identities: input.sweep_identities,
        results_digest: input.results_digest,
        jobbench_revision: input.jobbench_revision,
    }
}
