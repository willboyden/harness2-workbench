//! Reproducibility manifests: everything needed to re-run or audit an experiment.

use h2w_api::{Isolation, KeyValue, Manifest, ManifestJob, SweepIdentity};

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

fn sorted_pairs(mut v: Vec<KeyValue>) -> Vec<KeyValue> {
    v.sort_by(|a, b| a.key.cmp(&b.key).then_with(|| a.value.cmp(&b.value)));
    v.dedup();
    v
}

/// Assemble the manifest (sorting lists so equal inputs give byte-equal JSON).
///
/// Pins and models sort by key, environment names sort and deduplicate, jobs sort by
/// creation time then id, sweep identities by sweep.
#[must_use]
pub fn build(input: ManifestInput) -> Manifest {
    let mut env_names = input.env_names;
    env_names.sort();
    env_names.dedup();
    let mut jobs = input.jobs;
    jobs.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut sweep_identities = input.sweep_identities;
    sweep_identities.sort_by(|a, b| a.sweep.cmp(&b.sweep));
    Manifest {
        schema_version: SCHEMA_VERSION,
        generated_at: input.generated_at,
        studio_version: input.studio_version,
        experiment: input.experiment,
        harness2_commit: input.harness2_commit,
        dependency_pins: sorted_pairs(input.dependency_pins),
        models: sorted_pairs(input.models),
        env_names,
        isolation: input.isolation,
        network_policy: input.network_policy,
        jobs,
        sweep_identities,
        results_digest: input.results_digest,
        jobbench_revision: input.jobbench_revision,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use h2w_api::{JobKind, JobStatus};

    fn kv(k: &str, v: &str) -> KeyValue {
        KeyValue {
            key: k.into(),
            value: v.into(),
        }
    }

    fn job(id: &str, at: &str) -> ManifestJob {
        ManifestJob {
            id: id.into(),
            kind: JobKind::Run,
            status: JobStatus::Succeeded,
            argv: vec!["harness2".into()],
            created_at: at.into(),
            finished_at: None,
        }
    }

    #[test]
    fn equal_inputs_give_equal_json() {
        let a = ManifestInput {
            generated_at: "2026-10-23T00:00:00Z".into(),
            studio_version: "0.1.0".into(),
            experiment: "e".into(),
            harness2_commit: Some(crate::TESTED_HARNESS2_COMMIT.into()),
            dependency_pins: vec![kv("opencode", "x"), kv("harvey-labs", "y")],
            models: vec![kv("solver", "a"), kv("judge", "b")],
            env_names: vec!["B".into(), "A".into(), "B".into()],
            isolation: Isolation::Host,
            network_policy: "host".into(),
            jobs: vec![
                job("2", "2026-10-02T00:00:00Z"),
                job("1", "2026-10-01T00:00:00Z"),
            ],
            sweep_identities: vec![
                SweepIdentity {
                    sweep: "lab-oc-agg2".into(),
                    identity_json: "{}".into(),
                },
                SweepIdentity {
                    sweep: "lab-oc-agg1".into(),
                    identity_json: "{}".into(),
                },
            ],
            results_digest: "00".into(),
            jobbench_revision: None,
        };
        let mut b = a.clone();
        b.dependency_pins.reverse();
        b.models.reverse();
        b.env_names.reverse();
        b.jobs.reverse();
        b.sweep_identities.reverse();
        let ma = build(a);
        let mb = build(b);
        assert_eq!(
            serde_json::to_string(&ma).unwrap(),
            serde_json::to_string(&mb).unwrap()
        );
        assert_eq!(ma.env_names, vec!["A", "B"]);
        assert_eq!(ma.jobs[0].id, "1");
        assert_eq!(ma.dependency_pins[0].key, "harvey-labs");
        assert_eq!(ma.sweep_identities[0].sweep, "lab-oc-agg1");
        assert_eq!(ma.schema_version, SCHEMA_VERSION);
    }
}
