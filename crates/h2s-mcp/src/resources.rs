//! MCP resources: job logs, sweep reports and experiment manifests.
//!
//! URIs (contract §13): `h2s://jobs/{id}/log`, `h2s://experiments/{exp}/sweeps/{sweep}/report`
//! and `h2s://experiments/{exp}/manifest`.

use h2s_api::{Experiment, Job, LogChunk, LogStream, Manifest, Page, ScoreReport};
use rmcp::{
    ErrorData as McpError,
    model::{ReadResourceResult, Resource, ResourceContents, ResourceTemplate},
};
use serde::Serialize;

use crate::{client::ApiClient, output::MAX_TEXT_BYTES, token::Token};

const SCHEME: &str = "h2s://";
/// Lines of a job log a resource read returns (its tail).
const LOG_TAIL_LINES: u32 = 2000;
/// Jobs whose logs `resources/list` offers.
const LISTED_JOBS: u32 = 20;

pub(crate) fn templates() -> Vec<ResourceTemplate> {
    vec![
        ResourceTemplate::new("h2s://jobs/{id}/log", "job-log")
            .with_title("Job log")
            .with_description(
                "The last 2,000 lines of a Harness² Studio job's log (stdout, stderr and server \
                 lines; secret values are redacted by the server). Page through all of it with \
                 the get_job_log tool.",
            )
            .with_mime_type("text/plain"),
        ResourceTemplate::new(
            "h2s://experiments/{exp}/sweeps/{sweep}/report",
            "sweep-report",
        )
        .with_title("Sweep score report")
        .with_description(
            "A judged sweep's score report as JSON: per mode and k, graded and ungraded counts, \
             score and all-pass rate in PERCENT 0..100, base_score, selector choices, spend, \
             upstream notes, and the harness2-report text block.",
        )
        .with_mime_type("application/json"),
        ResourceTemplate::new("h2s://experiments/{exp}/manifest", "experiment-manifest")
            .with_title("Experiment manifest")
            .with_description(
                "An experiment's reproducibility manifest as JSON: versions, dependency pins, \
                 model ids, environment variable names, isolation, every job with its argv, \
                 sweep identities and a results digest.",
            )
            .with_mime_type("application/json"),
    ]
}

/// Concrete resources the caller can read now: every experiment's manifest, every judged
/// sweep's report and the logs of the most recent jobs. Failures leave a part out.
pub(crate) async fn list(api: &ApiClient, token: &Token) -> Vec<Resource> {
    let limit = [("limit", LISTED_JOBS.to_string())];
    let (experiments, jobs) = tokio::join!(
        api.get::<Vec<Experiment>>(token, &["experiments"], &[]),
        api.get::<Page<Job>>(token, &["jobs"], &limit),
    );
    let mut out = Vec::new();
    match experiments {
        Ok(experiments) => {
            for exp in experiments {
                out.push(
                    Resource::new(
                        format!("{SCHEME}experiments/{}/manifest", exp.name),
                        format!("manifest-{}", exp.name),
                    )
                    .with_title(format!("Manifest of experiment {}", exp.name))
                    .with_mime_type("application/json"),
                );
                for sweep in exp.sweeps.iter().filter(|s| s.headline.is_some()) {
                    out.push(
                        Resource::new(
                            format!(
                                "{SCHEME}experiments/{}/sweeps/{}/report",
                                exp.name, sweep.id
                            ),
                            format!("report-{}-{}", exp.name, sweep.id),
                        )
                        .with_title(format!("Report of sweep {} ({})", sweep.id, exp.name))
                        .with_mime_type("application/json"),
                    );
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "resources/list: cannot list experiments"),
    }
    match jobs {
        Ok(page) => {
            for job in page.items {
                out.push(
                    Resource::new(
                        format!("{SCHEME}jobs/{}/log", job.id),
                        format!("log-{}", job.id),
                    )
                    .with_title(format!(
                        "Log of {} job {} ({})",
                        job.kind.as_str(),
                        job.id,
                        job.status.as_str()
                    ))
                    .with_mime_type("text/plain"),
                );
            }
        }
        Err(e) => tracing::warn!(error = %e, "resources/list: cannot list jobs"),
    }
    out
}

/// What a resource URI names.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Target {
    JobLog(String),
    Report { experiment: String, sweep: String },
    Manifest(String),
}

/// Parse an `h2s://` URI. Segments are identifiers: letters, digits, `.`, `_`, `~`, `-`.
pub(crate) fn parse(uri: &str) -> Option<Target> {
    let rest = uri.strip_prefix(SCHEME)?;
    let segments: Vec<&str> = rest.split('/').collect();
    let valid = |s: &&str| {
        !s.is_empty()
            && *s != "."
            && *s != ".."
            && s.len() <= 256
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'-'))
    };
    if !segments.iter().all(valid) {
        return None;
    }
    match segments.as_slice() {
        ["jobs", id, "log"] => Some(Target::JobLog((*id).to_owned())),
        ["experiments", exp, "sweeps", sweep, "report"] => Some(Target::Report {
            experiment: (*exp).to_owned(),
            sweep: (*sweep).to_owned(),
        }),
        ["experiments", exp, "manifest"] => Some(Target::Manifest((*exp).to_owned())),
        _ => None,
    }
}

pub(crate) async fn read(
    api: &ApiClient,
    token: &Token,
    uri: &str,
) -> Result<ReadResourceResult, McpError> {
    let target = parse(uri).ok_or_else(|| {
        McpError::resource_not_found(
            format!(
                "unknown resource {uri:?}; readable URIs: h2s://jobs/{{id}}/log, \
                 h2s://experiments/{{exp}}/sweeps/{{sweep}}/report, \
                 h2s://experiments/{{exp}}/manifest"
            ),
            None,
        )
    })?;
    let contents = match target {
        Target::JobLog(id) => {
            let job: Job = api.get(token, &["jobs", &id], &[]).await?;
            let from = job.log_lines.saturating_sub(u64::from(LOG_TAIL_LINES));
            let query = [
                ("from", from.to_string()),
                ("limit", LOG_TAIL_LINES.to_string()),
            ];
            let chunk: LogChunk = api.get(token, &["jobs", &id, "log"], &query).await?;
            ResourceContents::text(render_log(&job, &chunk), uri)
        }
        Target::Report { experiment, sweep } => {
            let report: ScoreReport = api
                .get(
                    token,
                    &["experiments", &experiment, "sweeps", &sweep, "report"],
                    &[],
                )
                .await?;
            json_contents(&report, uri)?
        }
        Target::Manifest(experiment) => {
            let manifest: Manifest = api
                .get(token, &["experiments", &experiment, "manifest"], &[])
                .await?;
            json_contents(&manifest, uri)?
        }
    };
    Ok(ReadResourceResult::new(vec![contents]))
}

fn json_contents<T: Serialize>(value: &T, uri: &str) -> Result<ResourceContents, McpError> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| McpError::internal_error(format!("cannot encode {uri}: {e}"), None))?;
    Ok(ResourceContents::text(text, uri).with_mime_type("application/json"))
}

/// The log as text, newest lines kept when it exceeds the output cap.
fn render_log(job: &Job, chunk: &LogChunk) -> String {
    let first = chunk.lines.first().map_or(0, |l| l.n);
    let last = chunk.lines.last().map_or(0, |l| l.n);
    let mut body = String::new();
    for line in &chunk.lines {
        let stream = match line.stream {
            LogStream::Stdout => "out",
            LogStream::Stderr => "err",
            LogStream::System => "sys",
        };
        body.push_str(&format!("{} [{stream}] {}\n", line.ts, line.text));
    }
    let mut kept_from = first;
    if body.len() > MAX_TEXT_BYTES {
        let mut cut = body.len() - MAX_TEXT_BYTES;
        while !body.is_char_boundary(cut) {
            cut += 1;
        }
        // Start at a line boundary.
        if let Some(nl) = body[cut..].find('\n') {
            cut += nl + 1;
        }
        let dropped = body[..cut].matches('\n').count() as u64;
        kept_from = first + dropped;
        body.drain(..cut);
    }
    let header = if chunk.lines.is_empty() {
        format!(
            "# {} job {} ({}): no log lines yet\n",
            job.kind.as_str(),
            job.id,
            job.status.as_str()
        )
    } else {
        format!(
            "# {} job {} ({}): log lines {kept_from}-{last} of {} total; earlier lines: \
             get_job_log(job_id=\"{}\", from=0)\n",
            job.kind.as_str(),
            job.id,
            job.status.as_str(),
            job.log_lines,
            job.id
        )
    };
    format!("{header}{body}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_three_uri_forms() {
        assert_eq!(
            parse("h2s://jobs/0192-ab/log"),
            Some(Target::JobLog("0192-ab".into()))
        );
        assert_eq!(
            parse("h2s://experiments/demo/sweeps/demo-oc-agg2/report"),
            Some(Target::Report {
                experiment: "demo".into(),
                sweep: "demo-oc-agg2".into()
            })
        );
        assert_eq!(
            parse("h2s://experiments/e_1/manifest"),
            Some(Target::Manifest("e_1".into()))
        );
    }

    #[test]
    fn rejects_everything_else() {
        for bad in [
            "h2s://jobs//log",
            "h2s://jobs/../log",
            "h2s://jobs/a%2Fb/log",
            "h2s://jobs/a/log/extra",
            "h2s://experiments/x/sweeps/y",
            "file:///etc/passwd",
            "h2s://experiments/x y/manifest",
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn templates_match_the_contract() {
        let uris: Vec<String> = templates().into_iter().map(|t| t.uri_template).collect();
        assert_eq!(
            uris,
            vec![
                "h2s://jobs/{id}/log",
                "h2s://experiments/{exp}/sweeps/{sweep}/report",
                "h2s://experiments/{exp}/manifest"
            ]
        );
    }
}
