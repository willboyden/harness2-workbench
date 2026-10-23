//! Prometheus metrics (ARCHITECTURE.md §14), with bounded label sets: methods outside the
//! standard set become `OTHER`, routes are matched route templates, experiments beyond a
//! fixed number share the label `other`.

use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;

use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::{Histogram, exponential_buckets};
use prometheus_client::registry::Registry;

/// Distinct `experiment` label values kept before folding into `other`.
pub const MAX_EXPERIMENT_LABELS: usize = 50;

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct HttpLabels {
    pub method: &'static str,
    pub route: String,
    pub status: u16,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct RouteLabels {
    pub method: &'static str,
    pub route: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct JobLabels {
    pub kind: &'static str,
    pub status: &'static str,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct KindLabels {
    pub kind: &'static str,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct ExperimentLabels {
    pub experiment: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct ReasonLabels {
    pub reason: &'static str,
}

type HistogramFamily<L> = Family<L, Histogram, fn() -> Histogram>;

fn http_histogram() -> Histogram {
    Histogram::new(exponential_buckets(0.005, 2.0, 12))
}

fn job_histogram() -> Histogram {
    Histogram::new(exponential_buckets(1.0, 2.0, 18))
}

/// Every metric the server exports.
pub struct Metrics {
    registry: Registry,
    pub http_requests: Family<HttpLabels, Counter>,
    pub http_duration: HistogramFamily<RouteLabels>,
    pub jobs_total: Family<JobLabels, Counter>,
    pub jobs_running: Gauge,
    pub job_duration: HistogramFamily<KindLabels>,
    pub spend_usd: Family<ExperimentLabels, Counter<f64, AtomicU64>>,
    pub auth_failures: Family<ReasonLabels, Counter>,
    pub sse_connections: Gauge,
    experiments: Mutex<HashSet<String>>,
}

impl std::fmt::Debug for Metrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Metrics")
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

/// The method label: standard methods verbatim, anything else `OTHER`.
#[must_use]
pub fn method_label(m: &http::Method) -> &'static str {
    match *m {
        http::Method::GET => "GET",
        http::Method::HEAD => "HEAD",
        http::Method::POST => "POST",
        http::Method::PUT => "PUT",
        http::Method::PATCH => "PATCH",
        http::Method::DELETE => "DELETE",
        http::Method::OPTIONS => "OPTIONS",
        _ => "OTHER",
    }
}

impl Metrics {
    #[must_use]
    pub fn new() -> Metrics {
        let mut registry = Registry::default();
        let http_requests = Family::<HttpLabels, Counter>::default();
        let http_duration: HistogramFamily<RouteLabels> =
            Family::new_with_constructor(http_histogram as fn() -> Histogram);
        let jobs_total = Family::<JobLabels, Counter>::default();
        let jobs_running = Gauge::default();
        let job_duration: HistogramFamily<KindLabels> =
            Family::new_with_constructor(job_histogram as fn() -> Histogram);
        let spend_usd = Family::<ExperimentLabels, Counter<f64, AtomicU64>>::default();
        let auth_failures = Family::<ReasonLabels, Counter>::default();
        let sse_connections = Gauge::default();
        // Counters are registered without `_total`; the encoder appends it.
        registry.register(
            "h2s_http_requests",
            "HTTP requests by method, matched route and status",
            http_requests.clone(),
        );
        registry.register(
            "h2s_http_request_duration_seconds",
            "HTTP request latency until the response head",
            http_duration.clone(),
        );
        registry.register(
            "h2s_jobs",
            "Jobs that reached a terminal status",
            jobs_total.clone(),
        );
        registry.register("h2s_jobs_running", "Jobs running now", jobs_running.clone());
        registry.register(
            "h2s_job_duration_seconds",
            "Wall-clock duration of finished jobs",
            job_duration.clone(),
        );
        registry.register(
            "h2s_spend_usd",
            "Improver spend recorded in task summaries while jobs ran (USD)",
            spend_usd.clone(),
        );
        registry.register(
            "h2s_auth_failures",
            "Refused requests by reason",
            auth_failures.clone(),
        );
        registry.register(
            "h2s_sse_connections",
            "Open job event streams",
            sse_connections.clone(),
        );
        Metrics {
            registry,
            http_requests,
            http_duration,
            jobs_total,
            jobs_running,
            job_duration,
            spend_usd,
            auth_failures,
            sse_connections,
            experiments: Mutex::new(HashSet::new()),
        }
    }

    /// Text exposition (OpenMetrics).
    #[must_use]
    pub fn encode(&self) -> String {
        let mut out = String::new();
        if prometheus_client::encoding::text::encode(&mut out, &self.registry).is_err() {
            out.clear();
        }
        out
    }

    /// Record one HTTP request.
    pub fn observe_http(&self, method: &http::Method, route: &str, status: u16, secs: f64) {
        let method = method_label(method);
        self.http_requests
            .get_or_create(&HttpLabels {
                method,
                route: route.to_owned(),
                status,
            })
            .inc();
        self.http_duration
            .get_or_create(&RouteLabels {
                method,
                route: route.to_owned(),
            })
            .observe(secs);
    }

    /// Count a refused request.
    pub fn auth_failure(&self, reason: &'static str) {
        self.auth_failures
            .get_or_create(&ReasonLabels { reason })
            .inc();
    }

    /// Add observed spend for an experiment (label set bounded).
    pub fn add_spend(&self, experiment: &str, usd: f64) {
        if !(usd.is_finite() && usd > 0.0) {
            return;
        }
        let label = {
            let mut set = match self.experiments.lock() {
                Ok(s) => s,
                Err(p) => p.into_inner(),
            };
            if set.contains(experiment) || set.len() < MAX_EXPERIMENT_LABELS {
                set.insert(experiment.to_owned());
                experiment.to_owned()
            } else {
                "other".to_owned()
            }
        };
        self.spend_usd
            .get_or_create(&ExperimentLabels { experiment: label })
            .inc_by(usd);
    }

    /// Record a finished job.
    pub fn job_finished(&self, kind: &'static str, status: &'static str, secs: f64) {
        self.jobs_total
            .get_or_create(&JobLabels { kind, status })
            .inc();
        self.job_duration
            .get_or_create(&KindLabels { kind })
            .observe(secs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposition_has_the_documented_names() {
        let m = Metrics::new();
        m.observe_http(&http::Method::GET, "/api/v1/jobs", 200, 0.01);
        m.observe_http(
            &http::Method::from_bytes(b"BREW").unwrap(),
            "unmatched",
            405,
            0.01,
        );
        m.job_finished("demo", "succeeded", 3.0);
        m.jobs_running.set(1);
        m.add_spend("e1", 0.25);
        m.auth_failure("missing");
        m.sse_connections.inc();
        let text = m.encode();
        for name in [
            "h2s_http_requests_total{",
            "h2s_http_request_duration_seconds_bucket{",
            "h2s_jobs_total{kind=\"demo\",status=\"succeeded\"} 1",
            "h2s_jobs_running 1",
            "h2s_job_duration_seconds_count{kind=\"demo\"} 1",
            "h2s_spend_usd_total{experiment=\"e1\"} 0.25",
            "h2s_auth_failures_total{reason=\"missing\"} 1",
            "h2s_sse_connections 1",
            "method=\"OTHER\"",
        ] {
            assert!(text.contains(name), "missing {name} in\n{text}");
        }
    }

    #[test]
    fn experiment_labels_are_bounded() {
        let m = Metrics::new();
        for i in 0..(MAX_EXPERIMENT_LABELS + 20) {
            m.add_spend(&format!("e{i}"), 1.0);
        }
        let text = m.encode();
        let n = text
            .lines()
            .filter(|l| l.starts_with("h2s_spend_usd_total{"))
            .count();
        assert_eq!(n, MAX_EXPERIMENT_LABELS + 1);
        assert!(text.contains("experiment=\"other\"} 20"), "{text}");
    }
}
