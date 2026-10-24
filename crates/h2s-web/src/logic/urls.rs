//! URL building for API calls (`api::*`) and in-app links (`route::*`). Every dynamic path
//! segment and query value is percent-encoded here, so no caller concatenates raw input.

use std::fmt::Write as _;

use h2s_api::{ExportTarget, Mode};

/// Percent-encode a path segment or query value, keeping only RFC 3986 unreserved bytes.
pub(crate) fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// A query string from `(key, value)` pairs, skipping empty values: `?a=1&b=x%20y`.
pub(crate) fn query(pairs: &[(&str, &str)]) -> String {
    let mut out = String::new();
    for (k, v) in pairs.iter().filter(|(_, v)| !v.is_empty()) {
        out.push(if out.is_empty() { '?' } else { '&' });
        out.push_str(&enc(k));
        out.push('=');
        out.push_str(&enc(v));
    }
    out
}

/// The `<experiment>/<sweep>/<mode>` reference `GET /compare` takes for each side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SweepRef {
    pub(crate) experiment: String,
    pub(crate) sweep: String,
    pub(crate) mode: Mode,
}

impl SweepRef {
    pub(crate) fn to_param(&self) -> String {
        format!("{}/{}/{}", self.experiment, self.sweep, self.mode.as_cli())
    }

    /// Parse `exp/sweep/mode` (mode as `parallel`/`sequential` or `agg`/`seq`).
    pub(crate) fn parse(s: &str) -> Option<SweepRef> {
        let mut parts = s.split('/');
        let (experiment, sweep, mode) = (parts.next()?, parts.next()?, parts.next()?);
        if parts.next().is_some() || experiment.is_empty() || sweep.is_empty() {
            return None;
        }
        Some(SweepRef {
            experiment: experiment.to_owned(),
            sweep: sweep.to_owned(),
            mode: Mode::from_token(mode)?,
        })
    }
}

/// REST API paths (all under `/api/v1`).
pub(crate) mod api {
    use super::{ExportTarget, Mode, SweepRef, enc, query};
    use h2s_api::API_PREFIX;

    fn p(rest: &str) -> String {
        format!("{API_PREFIX}{rest}")
    }

    pub(crate) fn login() -> String {
        p("/auth/login")
    }
    pub(crate) fn logout() -> String {
        p("/auth/logout")
    }
    pub(crate) fn me() -> String {
        p("/auth/me")
    }
    pub(crate) fn system_info() -> String {
        p("/system/info")
    }
    pub(crate) fn doctor(refresh: bool) -> String {
        p(&format!(
            "/system/doctor{}",
            query(&[("refresh", if refresh { "true" } else { "" })])
        ))
    }
    pub(crate) fn benchmarks() -> String {
        p("/benchmarks")
    }
    pub(crate) fn profiles() -> String {
        p("/profiles")
    }
    pub(crate) fn profile(name: &str) -> String {
        p(&format!("/profiles/{}", enc(name)))
    }
    pub(crate) fn profile_check(name: &str) -> String {
        p(&format!("/profiles/{}/check", enc(name)))
    }
    pub(crate) fn secrets() -> String {
        p("/secrets")
    }
    pub(crate) fn secret(name: &str) -> String {
        p(&format!("/secrets/{}", enc(name)))
    }
    pub(crate) fn tokens() -> String {
        p("/tokens")
    }
    pub(crate) fn token(id: &str) -> String {
        p(&format!("/tokens/{}", enc(id)))
    }
    pub(crate) fn audit_log(limit: u32, cursor: Option<&str>) -> String {
        let limit = limit.to_string();
        p(&format!(
            "/audit{}",
            query(&[("limit", &limit), ("cursor", cursor.unwrap_or_default())])
        ))
    }
    pub(crate) fn audit_verify() -> String {
        p("/audit/verify")
    }
    pub(crate) fn experiments(archived: bool) -> String {
        p(&format!(
            "/experiments{}",
            query(&[("archived", if archived { "true" } else { "" })])
        ))
    }
    pub(crate) fn experiment(exp: &str) -> String {
        p(&format!("/experiments/{}", enc(exp)))
    }
    pub(crate) fn manifest(exp: &str) -> String {
        format!("{}/manifest", experiment(exp))
    }
    fn sweep(exp: &str, sweep: &str) -> String {
        format!("{}/sweeps/{}", experiment(exp), enc(sweep))
    }
    pub(crate) fn tasks(
        exp: &str,
        sw: &str,
        domain: &str,
        status: &str,
        limit: u32,
        cursor: Option<&str>,
    ) -> String {
        let limit = limit.to_string();
        format!(
            "{}/tasks{}",
            sweep(exp, sw),
            query(&[
                ("domain", domain),
                ("status", status),
                ("limit", &limit),
                ("cursor", cursor.unwrap_or_default()),
            ])
        )
    }
    pub(crate) fn task(exp: &str, sw: &str, task: &str) -> String {
        format!("{}/tasks/{}", sweep(exp, sw), enc(task))
    }
    fn task_mode(exp: &str, sw: &str, t: &str, mode: Mode) -> String {
        format!("{}/modes/{}", task(exp, sw, t), mode.as_cli())
    }
    pub(crate) fn harness(exp: &str, sw: &str, t: &str, mode: Mode, label: &str) -> String {
        format!("{}/harnesses/{}", task_mode(exp, sw, t, mode), enc(label))
    }
    pub(crate) fn diff(exp: &str, sw: &str, t: &str, mode: Mode, from: &str, to: &str) -> String {
        format!(
            "{}/diff{}",
            task_mode(exp, sw, t, mode),
            query(&[("from", from), ("to", to)])
        )
    }
    pub(crate) fn export(
        exp: &str,
        sw: &str,
        t: &str,
        mode: Mode,
        label: &str,
        target: ExportTarget,
        tar_gz: bool,
    ) -> String {
        let target = match target {
            ExportTarget::Raw => "raw",
            ExportTarget::Opencode => "opencode",
            ExportTarget::Codex => "codex",
        };
        format!(
            "{}/export{}",
            harness(exp, sw, t, mode, label),
            query(&[
                ("target", target),
                ("format", if tar_gz { "tar.gz" } else { "json" })
            ])
        )
    }
    pub(crate) fn rollout_files(
        exp: &str,
        sw: &str,
        t: &str,
        mode: Mode,
        label: &str,
        path: &str,
    ) -> String {
        format!(
            "{}/rollouts/{}/files{}",
            task_mode(exp, sw, t, mode),
            enc(label),
            query(&[("path", path)])
        )
    }
    pub(crate) fn rollout_file(
        exp: &str,
        sw: &str,
        t: &str,
        mode: Mode,
        label: &str,
        path: &str,
    ) -> String {
        format!(
            "{}/rollouts/{}/file{}",
            task_mode(exp, sw, t, mode),
            enc(label),
            query(&[("path", path)])
        )
    }
    pub(crate) fn scores(exp: &str, sw: &str) -> String {
        format!("{}/scores", sweep(exp, sw))
    }
    pub(crate) fn report(exp: &str, sw: &str) -> String {
        format!("{}/report", sweep(exp, sw))
    }
    pub(crate) fn library(exp: &str, sw: &str) -> String {
        format!("{}/library", sweep(exp, sw))
    }
    pub(crate) fn sweep_audit(exp: &str, sw: &str) -> String {
        format!("{}/audit", sweep(exp, sw))
    }
    pub(crate) fn sweep_audit_part(exp: &str, sw: &str, session: &str, part: &str) -> String {
        format!("{}/audit/{}/{}", sweep(exp, sw), enc(session), enc(part))
    }
    pub(crate) fn compare(a: &SweepRef, b: &SweepRef) -> String {
        p(&format!(
            "/compare{}",
            query(&[("a", &a.to_param()), ("b", &b.to_param())])
        ))
    }
    pub(crate) fn estimate() -> String {
        p("/estimate")
    }
    pub(crate) fn splits() -> String {
        p("/splits")
    }
    pub(crate) fn jobs(
        status: &str,
        kind: &str,
        experiment: &str,
        limit: u32,
        cursor: Option<&str>,
    ) -> String {
        let limit = limit.to_string();
        p(&format!(
            "/jobs{}",
            query(&[
                ("status", status),
                ("kind", kind),
                ("experiment", experiment),
                ("limit", &limit),
                ("cursor", cursor.unwrap_or_default()),
            ])
        ))
    }
    pub(crate) fn create_job() -> String {
        p("/jobs")
    }
    pub(crate) fn job(id: &str) -> String {
        p(&format!("/jobs/{}", enc(id)))
    }
    pub(crate) fn job_cancel(id: &str) -> String {
        format!("{}/cancel", job(id))
    }
    pub(crate) fn job_log(id: &str, from: u64, limit: u32) -> String {
        format!("{}/log?from={from}&limit={limit}", job(id))
    }
    pub(crate) fn job_events(id: &str) -> String {
        format!("{}/events", job(id))
    }
}

/// In-app routes (client-side paths).
pub(crate) mod route {
    use super::{enc, query};

    pub(crate) fn experiment(exp: &str) -> String {
        format!("/experiments/{}", enc(exp))
    }
    pub(crate) fn sweep(exp: &str, sweep: &str) -> String {
        format!("{}/sweeps/{}", experiment(exp), enc(sweep))
    }
    pub(crate) fn sweep_tab(exp: &str, sw: &str, tab: &str) -> String {
        format!("{}{}", sweep(exp, sw), query(&[("tab", tab)]))
    }
    pub(crate) fn task(exp: &str, sw: &str, task: &str) -> String {
        format!("{}/tasks/{}", sweep(exp, sw), enc(task))
    }
    pub(crate) fn job(id: &str) -> String {
        format!("/jobs/{}", enc(id))
    }
    pub(crate) fn new_run_for(exp: &str) -> String {
        format!("/new{}", query(&[("experiment", exp)]))
    }
    pub(crate) fn help_term(id: &str) -> String {
        format!("/help#term-{}", enc(id))
    }
    /// `/login?next=<path>`; a path that fails [`clean_next`] is dropped, not carried along.
    pub(crate) fn login_next(next: &str) -> String {
        format!(
            "/login{}",
            query(&[("next", clean_next(next).unwrap_or(""))])
        )
    }

    /// The pure half of the `next` check after sign-in (no open redirects): an absolute path
    /// inside this app, and never the sign-in page itself. Refused:
    /// - control characters and whitespace anywhere: the URL parser deletes tabs and newlines,
    ///   so "/\t/evil.example" would become the protocol-relative "//evil.example";
    /// - backslashes (the parser reads them as "/") and a leading "//";
    /// - dot segments, also percent-encoded ("%2e"), and empty segments other than a trailing
    ///   slash: normalising "/..//evil" also yields "//evil".
    ///
    /// The browser's own URL parser has the last word (`nav::follow_next`).
    pub(crate) fn clean_next(next: &str) -> Option<&str> {
        if !next.starts_with('/')
            || next.starts_with("//")
            || next.starts_with("/login")
            || next.contains('\\')
            || next.chars().any(|c| c.is_control() || c.is_whitespace())
        {
            return None;
        }
        let path = next.split(['?', '#']).next().unwrap_or_default();
        let segments: Vec<&str> = path.get(1..).unwrap_or_default().split('/').collect();
        let last = segments.len().saturating_sub(1);
        let bad = segments.iter().enumerate().any(|(i, seg)| {
            let seg = seg.to_ascii_lowercase();
            (seg.is_empty() && i != last)
                || matches!(
                    seg.as_str(),
                    "." | "%2e" | ".." | ".%2e" | "%2e." | "%2e%2e"
                )
        });
        (!bad).then_some(next)
    }

    /// `next` when it passes [`clean_next`], else "/".
    pub(crate) fn safe_next(next: Option<&str>) -> String {
        next.and_then(clean_next).unwrap_or("/").to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding() {
        assert_eq!(enc("lab-oc-agg1_s7.x~"), "lab-oc-agg1_s7.x~");
        assert_eq!(enc("a b/c?d&e"), "a%20b%2Fc%3Fd%26e");
        assert_eq!(enc("é"), "%C3%A9");
        assert_eq!(
            query(&[("a", "1"), ("b", ""), ("c", "x y")]),
            "?a=1&c=x%20y"
        );
        assert_eq!(query(&[("a", "")]), "");
    }

    #[test]
    fn api_paths() {
        assert_eq!(
            api::experiment("lab pilot"),
            "/api/v1/experiments/lab%20pilot"
        );
        assert_eq!(
            api::profile_check("vertex-flash"),
            "/api/v1/profiles/vertex-flash/check"
        );
        assert_eq!(
            api::tasks("e", "s", "Contracts & Deals", "", 50, Some("c1")),
            "/api/v1/experiments/e/sweeps/s/tasks?domain=Contracts%20%26%20Deals&limit=50&cursor=c1"
        );
        assert_eq!(
            api::harness("e", "s", "t__1", Mode::Sequential, "c1"),
            "/api/v1/experiments/e/sweeps/s/tasks/t__1/modes/sequential/harnesses/c1"
        );
        assert_eq!(
            api::diff("e", "s", "t", Mode::Parallel, "base", "agg1"),
            "/api/v1/experiments/e/sweeps/s/tasks/t/modes/parallel/diff?from=base&to=agg1"
        );
        assert_eq!(
            api::export(
                "e",
                "s",
                "t",
                Mode::Parallel,
                "agg1",
                ExportTarget::Codex,
                true
            ),
            "/api/v1/experiments/e/sweeps/s/tasks/t/modes/parallel/harnesses/agg1/export?target=codex&format=tar.gz"
        );
        assert_eq!(
            api::rollout_file("e", "s", "t", Mode::Parallel, "base1", "out/a b.md"),
            "/api/v1/experiments/e/sweeps/s/tasks/t/modes/parallel/rollouts/base1/file?path=out%2Fa%20b.md"
        );
        assert_eq!(
            api::sweep_audit_part("e", "s", "inv1.a1.ff", "mission"),
            "/api/v1/experiments/e/sweeps/s/audit/inv1.a1.ff/mission"
        );
        assert_eq!(
            api::job_log("j1", 10, 500),
            "/api/v1/jobs/j1/log?from=10&limit=500"
        );
        assert_eq!(
            api::jobs("running", "", "", 20, None),
            "/api/v1/jobs?status=running&limit=20"
        );
        assert_eq!(api::doctor(true), "/api/v1/system/doctor?refresh=true");
        assert_eq!(api::doctor(false), "/api/v1/system/doctor");
        assert_eq!(api::audit_log(50, None), "/api/v1/audit?limit=50");
    }

    #[test]
    fn compare_refs() {
        let r = SweepRef::parse("exp/lab-oc-agg1/agg").unwrap();
        assert_eq!(r.mode, Mode::Parallel);
        assert_eq!(r.to_param(), "exp/lab-oc-agg1/parallel");
        assert!(SweepRef::parse("exp/sweep").is_none());
        assert!(SweepRef::parse("exp/sweep/both").is_none());
        assert!(SweepRef::parse("a/b/parallel/x").is_none());
        let b = SweepRef::parse("x/y/sequential").unwrap();
        assert_eq!(
            api::compare(&r, &b),
            "/api/v1/compare?a=exp%2Flab-oc-agg1%2Fparallel&b=x%2Fy%2Fsequential"
        );
    }

    #[test]
    fn routes_and_redirects() {
        assert_eq!(
            route::sweep_tab("e", "s", "report"),
            "/experiments/e/sweeps/s?tab=report"
        );
        assert_eq!(route::new_run_for("lab"), "/new?experiment=lab");
        assert_eq!(route::login_next("/jobs/1"), "/login?next=%2Fjobs%2F1");
        assert_eq!(route::login_next("//evil.example"), "/login");
        assert_eq!(route::login_next("/login?next=x"), "/login");
        assert_eq!(route::safe_next(Some("/jobs")), "/jobs");
        assert_eq!(route::safe_next(Some("//evil.example")), "/");
        assert_eq!(route::safe_next(Some("https://evil.example")), "/");
        assert_eq!(route::safe_next(Some("/\\evil")), "/");
        assert_eq!(route::safe_next(None), "/");
        assert_eq!(
            route::safe_next(Some("/experiments/e?tab=report#top")),
            "/experiments/e?tab=report#top"
        );
        assert_eq!(route::safe_next(Some("/jobs/")), "/jobs/");
        assert_eq!(route::safe_next(Some("/login")), "/");
    }

    #[test]
    fn next_cannot_leave_the_origin() {
        // The URL parser deletes tabs and newlines, so these would become "//evil...".
        for bad in [
            "/\t/evil.example",
            "/\n/evil.example",
            "/\r//evil",
            " //evil",
            "/\u{0}x",
            "/\u{7f}x",
            "/\u{2028}/evil",
            "/ /evil",
            // Dot segments collapse: "/..//evil" and "/.//evil" both normalise to "//evil".
            "/..//evil.example",
            "/.//evil.example",
            "/%2e%2e//evil.example",
            "/%2E//evil.example",
            "/.%2e//evil.example",
            "/a//b",
            "\\/evil.example",
            "/\\/evil.example",
            "",
            "evil.example",
            "javascript:alert(1)",
        ] {
            assert_eq!(route::safe_next(Some(bad)), "/", "{bad:?}");
            assert_eq!(route::login_next(bad), "/login", "{bad:?}");
        }
        // A literal "%09" stays a same-origin path segment: nothing decodes it into a tab.
        let kept = route::safe_next(Some("/%09/evil"));
        assert_eq!(kept, "/%09/evil");
        assert!(!kept.starts_with("//"));
    }
}
