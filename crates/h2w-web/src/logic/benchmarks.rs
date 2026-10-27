//! Whether this server sets up and runs a benchmark (`BenchmarkStatus.unavailable`): the words
//! the Benchmarks and New run pages use, and the lookups behind them.

use h2w_api::{Bench, BenchmarkStatus, BenchmarkUnavailable, UnavailableReason, WbSubset};

/// The badge that replaces a card's setup state.
pub(crate) fn badge(u: &BenchmarkUnavailable) -> &'static str {
    match u.reason {
        UnavailableReason::NotOffered => "not on this server",
        UnavailableReason::Isolation => "needs host isolation",
    }
}

/// The title of the notice that explains it.
pub(crate) fn title(u: &BenchmarkUnavailable) -> &'static str {
    match u.reason {
        UnavailableReason::NotOffered => "Not on this server",
        UnavailableReason::Isolation => "Needs host isolation",
    }
}

/// The New run wizard's problem for a benchmark it cannot start here.
pub(crate) fn problem(u: &BenchmarkUnavailable) -> String {
    match u.reason {
        UnavailableReason::NotOffered => format!("Not on this server: {}", u.message),
        UnavailableReason::Isolation => u.message.clone(),
    }
}

/// The link to the Workbench that runs it: only an `http(s)` URL (the server validates the
/// setting too; this keeps any other scheme out of an `href`).
pub(crate) fn link(u: &BenchmarkUnavailable) -> Option<&str> {
    u.url.as_deref().filter(|s| {
        let lower = s.to_ascii_lowercase();
        lower.starts_with("https://") || lower.starts_with("http://")
    })
}

/// The status entry of `bench` (for WorkBuddy-Bench, of one subset).
pub(crate) fn entry(
    list: &[BenchmarkStatus],
    bench: Bench,
    subset: Option<WbSubset>,
) -> Option<&BenchmarkStatus> {
    list.iter()
        .find(|s| s.bench == bench && (subset.is_none() || s.subset == subset))
}

/// The benchmarks this server runs (a split is only offered for those).
pub(crate) fn available(list: Vec<BenchmarkStatus>) -> Vec<BenchmarkStatus> {
    list.into_iter()
        .filter(|b| b.unavailable.is_none())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(
        bench: Bench,
        subset: Option<WbSubset>,
        u: Option<BenchmarkUnavailable>,
    ) -> BenchmarkStatus {
        BenchmarkStatus {
            bench,
            subset,
            display_name: bench.display_name().to_owned(),
            installed: false,
            at_pinned_commit: None,
            data_present: false,
            domains: Vec::new(),
            requirements: Vec::new(),
            upstream_url: String::new(),
            license_note: String::new(),
            description: String::new(),
            unavailable: u,
        }
    }

    fn unavailable(reason: UnavailableReason, url: Option<&str>) -> BenchmarkUnavailable {
        BenchmarkUnavailable {
            reason,
            message: "WorkBuddy-Bench runs on the sandbox Workbench.".to_owned(),
            url: url.map(str::to_owned),
        }
    }

    #[test]
    fn words_follow_the_reason() {
        let n = unavailable(UnavailableReason::NotOffered, None);
        let i = unavailable(UnavailableReason::Isolation, None);
        assert_eq!(badge(&n), "not on this server");
        assert_eq!(badge(&i), "needs host isolation");
        assert_eq!(title(&n), "Not on this server");
        assert_eq!(title(&i), "Needs host isolation");
        assert_eq!(
            problem(&n),
            "Not on this server: WorkBuddy-Bench runs on the sandbox Workbench."
        );
        assert_eq!(problem(&i), i.message);
    }

    #[test]
    fn only_http_links_are_shown() {
        let ok = unavailable(
            UnavailableReason::NotOffered,
            Some("https://wb.example.com/"),
        );
        assert_eq!(link(&ok), Some("https://wb.example.com/"));
        let plain = unavailable(
            UnavailableReason::NotOffered,
            Some("HTTP://workbench.example.org/"),
        );
        assert_eq!(link(&plain), Some("HTTP://workbench.example.org/"));
        for bad in [
            "javascript:alert(1)",
            "data:text/html,x",
            "//evil.example",
            "ftp://x",
        ] {
            let u = unavailable(UnavailableReason::NotOffered, Some(bad));
            assert_eq!(link(&u), None, "{bad}");
        }
        assert_eq!(link(&unavailable(UnavailableReason::Isolation, None)), None);
    }

    #[test]
    fn lookups_and_filtering() {
        let list = vec![
            status(Bench::Lab, None, None),
            status(
                Bench::Wb,
                Some(WbSubset::Office),
                Some(unavailable(UnavailableReason::Isolation, None)),
            ),
            status(
                Bench::Wb,
                Some(WbSubset::Web),
                Some(unavailable(UnavailableReason::Isolation, None)),
            ),
        ];
        assert_eq!(
            entry(&list, Bench::Lab, None).map(|s| s.bench),
            Some(Bench::Lab)
        );
        let web = entry(&list, Bench::Wb, Some(WbSubset::Web)).unwrap();
        assert_eq!(web.subset, Some(WbSubset::Web));
        assert!(entry(&list, Bench::Jb, None).is_none());
        let left = available(list);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].bench, Bench::Lab);
    }
}
