//! Environment checks: run `h2s_core::doctor::PROBES` (argv only, cleared environment,
//! 10 s timeout each, all in parallel) and let `h2s_core::doctor::evaluate` turn the
//! results into checks with remedies. Reports are cached for 60 s.

use std::path::Path;
use std::time::{Duration, Instant};

use h2s_api::{DoctorReport, Harness2Status, Isolation};
use h2s_core::doctor::{DoctorContext, PROBES, Probe, ProbeResult};
use nix::sys::signal::Signal;

/// How long a report is reused.
pub const CACHE_TTL: Duration = Duration::from_secs(60);
/// Per-probe limit.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const OUTPUT_CAP: usize = 16 * 1024;

/// Run one probe.
pub async fn run_probe(p: &Probe) -> ProbeResult {
    let mut cmd = tokio::process::Command::new(p.program);
    cmd.args(p.args)
        .env_clear()
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    // The same variables jobs get, so a probe finds what a job would (e.g. uv's managed
    // Python), plus what the container runtime CLI needs.
    for (name, value) in std::env::vars_os() {
        if name.to_str().is_some_and(|n| {
            h2s_core::env::is_passed_through(n)
                || crate::jobs::container::RUNTIME_ENV_PASSTHROUGH.contains(&n)
        }) {
            cmd.env(name, value);
        }
    }
    let not_found = |found: bool, err: String| ProbeResult {
        id: p.id.to_owned(),
        found,
        exit_code: None,
        stdout: String::new(),
        stderr: err,
    };
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return not_found(false, String::new());
        }
        Err(e) => return not_found(true, format!("could not run {}: {e}", p.program)),
    };
    let pid = child.id().and_then(|x| i32::try_from(x).ok());
    match tokio::time::timeout(PROBE_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(out)) => {
            let cap = |b: &[u8]| {
                let mut s = String::from_utf8_lossy(b).into_owned();
                if s.len() > OUTPUT_CAP {
                    s = crate::util::truncate_chars(&s, OUTPUT_CAP);
                }
                s
            };
            ProbeResult {
                id: p.id.to_owned(),
                found: true,
                exit_code: out.status.code(),
                stdout: cap(&out.stdout),
                stderr: cap(&out.stderr),
            }
        }
        Ok(Err(e)) => not_found(true, format!("{} failed: {e}", p.program)),
        Err(_) => {
            if let Some(pid) = pid {
                let _ = crate::jobs::procfs::signal_group(pid, Signal::SIGKILL);
            }
            not_found(true, format!("{} did not answer within 10 s", p.program))
        }
    }
}

/// Run every probe concurrently.
pub async fn run_probes() -> Vec<ProbeResult> {
    futures::future::join_all(PROBES.iter().map(run_probe)).await
}

/// Free bytes on the filesystem holding `path`.
#[must_use]
pub fn free_disk_bytes(path: &Path) -> Option<u64> {
    let st = nix::sys::statvfs::statvfs(path).ok()?;
    // The field types differ between 32- and 64-bit targets.
    #[allow(clippy::useless_conversion)]
    let (avail, frag) = (
        u64::try_from(st.blocks_available()).ok()?,
        u64::try_from(st.fragment_size()).ok()?,
    );
    avail.checked_mul(frag)
}

/// Facts for `evaluate` that need no probing.
#[derive(Debug, Clone)]
pub struct ContextFacts {
    pub harness2: Harness2Status,
    pub isolation: Isolation,
    pub data_dir: std::path::PathBuf,
    pub uses_vertex: bool,
    /// The container settings when isolation is `container` (for the jobs network check).
    pub container: Option<crate::config::ContainerSection>,
}

/// A report from fresh probes.
pub async fn fresh_report(facts: ContextFacts) -> DoctorReport {
    let results = run_probes().await;
    let ctx = DoctorContext {
        free_disk_bytes: free_disk_bytes(&facts.data_dir),
        harness2: facts.harness2,
        isolation: facts.isolation,
        uses_vertex: facts.uses_vertex,
        generated_at: crate::util::now_rfc3339(),
    };
    let mut report = h2s_core::doctor::evaluate(&results, &ctx);
    if let Some(c) = &facts.container {
        let runner = crate::jobs::container::SystemRunner;
        let mut extra = vec![crate::jobs::container::runner_image_check(&runner, c).await];
        extra.extend(crate::jobs::container::network_check(&runner, c).await);
        for check in extra {
            report.ok = report.ok && check.level != h2s_api::CheckLevel::Fail;
            report.checks.push(check);
        }
    }
    report
}

/// The cached doctor.
#[derive(Debug, Default)]
pub struct DoctorService {
    cache: tokio::sync::Mutex<Option<(Instant, DoctorReport)>>,
}

impl DoctorService {
    /// The cached report, or a fresh one when it is older than [`CACHE_TTL`] or `refresh`.
    /// Concurrent callers share one refresh.
    pub async fn report(
        &self,
        refresh: bool,
        facts: impl std::future::Future<Output = ContextFacts>,
    ) -> DoctorReport {
        let mut cache = self.cache.lock().await;
        if !refresh
            && let Some((at, r)) = cache.as_ref()
            && at.elapsed() < CACHE_TTL
        {
            return r.clone();
        }
        let r = fresh_report(facts.await).await;
        *cache = Some((Instant::now(), r.clone()));
        r
    }

    /// The cached report without probing, when one exists.
    pub async fn cached(&self) -> Option<DoctorReport> {
        self.cache.lock().await.as_ref().map(|(_, r)| r.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_programs_are_not_found() {
        let p = Probe {
            id: "nope",
            program: "h2s-definitely-not-installed-xyz",
            args: &["--version"],
        };
        let r = run_probe(&p).await;
        assert!(!r.found);
        assert_eq!(r.id, "nope");
    }

    #[tokio::test]
    async fn present_programs_report_output() {
        let p = Probe {
            id: "sh",
            program: "sh",
            args: &["-c", "echo hello; exit 3"],
        };
        let r = run_probe(&p).await;
        assert!(r.found);
        assert_eq!(r.exit_code, Some(3));
        assert_eq!(r.stdout.trim(), "hello");
    }

    #[test]
    fn disk_space_is_readable() {
        assert!(free_disk_bytes(Path::new("/")).is_some());
    }
}
