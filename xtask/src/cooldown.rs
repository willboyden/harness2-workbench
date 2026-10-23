//! Dependency cooldown gate (docs/SUPPLY-CHAIN.md).
//!
//! Every registry package in `Cargo.lock` must have been published at least `days` days ago,
//! according to the `pubtime` field of the crates.io sparse index. The only exceptions are
//! listed in `supply-chain/cooldown-exceptions.toml` (security fixes), each with an expiry
//! date; an expired or unused exception is reported so the list stays honest.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

struct Locked {
    name: String,
    version: String,
}

struct Exception {
    krate: String,
    version: String,
    reason: String,
    expires: String,
}

pub(crate) fn check(root: &Path, days: i64) -> Result<()> {
    let lock = std::fs::read_to_string(root.join("Cargo.lock")).context("reading Cargo.lock")?;
    let packages = parse_lock(&lock);
    let exceptions = parse_exceptions(
        &std::fs::read_to_string(root.join("supply-chain/cooldown-exceptions.toml"))
            .unwrap_or_default(),
    );
    let today = today()?;
    let cutoff = shift_days(&today, -days)?;

    let mut cache: BTreeMap<String, String> = BTreeMap::new();
    let mut young = Vec::new();
    let mut missing = Vec::new();
    let mut used = vec![false; exceptions.len()];
    for p in &packages {
        let body = match cache.get(&p.name) {
            Some(b) => b.clone(),
            None => {
                let b = fetch_index(&p.name)?;
                cache.insert(p.name.clone(), b.clone());
                b
            }
        };
        let Some(pubtime) = pubtime_of(&body, &p.version) else {
            missing.push(format!("{} {}", p.name, p.version));
            continue;
        };
        let date = pubtime.get(..10).unwrap_or("").to_owned();
        if date.as_str() > cutoff.as_str() {
            if let Some(i) = exceptions
                .iter()
                .position(|e| e.krate == p.name && e.version == p.version)
            {
                used[i] = true;
                continue;
            }
            young.push(format!("{} {} (published {date})", p.name, p.version));
        }
    }

    eprintln!(
        "cooldown: {} locked registry packages, minimum age {days} days (cutoff {cutoff})",
        packages.len()
    );
    for (i, e) in exceptions.iter().enumerate() {
        if !used[i] {
            eprintln!(
                "  note: exception {} {} is not needed any more (remove it)",
                e.krate, e.version
            );
        } else if e.expires.as_str() < today.as_str() {
            eprintln!(
                "  note: exception {} {} expired on {} ({})",
                e.krate, e.version, e.expires, e.reason
            );
        }
    }
    for m in &missing {
        eprintln!("  ? no pubtime in the index for {m}");
    }
    if !young.is_empty() {
        for y in &young {
            eprintln!("  YOUNG {y}");
        }
        bail!(
            "{} package(s) are younger than {days} days; pin older versions with \
             `cargo update -p <name>@<version> --precise <older>` or, for a security fix, \
             add an exception",
            young.len()
        );
    }
    eprintln!("cooldown: ok");
    Ok(())
}

/// Registry packages from Cargo.lock (`[[package]]` blocks with a registry source).
fn parse_lock(text: &str) -> Vec<Locked> {
    let mut out = Vec::new();
    for block in text.split("[[package]]").skip(1) {
        let field = |key: &str| {
            block.lines().find_map(|l| {
                let l = l.trim();
                l.strip_prefix(key)
                    .and_then(|rest| rest.trim_start().strip_prefix('='))
                    .map(|v| v.trim().trim_matches('"').to_owned())
            })
        };
        let (Some(name), Some(version), Some(source)) =
            (field("name"), field("version"), field("source"))
        else {
            continue;
        };
        if source.starts_with("registry+") {
            out.push(Locked { name, version });
        }
    }
    out
}

fn parse_exceptions(text: &str) -> Vec<Exception> {
    let mut out = Vec::new();
    for block in text.split("[[exception]]").skip(1) {
        let field = |key: &str| {
            block
                .lines()
                .find_map(|l| {
                    let l = l.trim();
                    l.strip_prefix(key)
                        .and_then(|rest| rest.trim_start().strip_prefix('='))
                        .map(|v| v.trim().trim_matches('"').to_owned())
                })
                .unwrap_or_default()
        };
        out.push(Exception {
            krate: field("crate"),
            version: field("version"),
            reason: field("reason"),
            expires: field("expires"),
        });
    }
    out
}

/// The sparse-index file of a crate, via curl (keeps xtask free of an HTTP stack).
fn fetch_index(name: &str) -> Result<String> {
    let n = name.to_ascii_lowercase();
    let path = match n.len() {
        1 => format!("1/{n}"),
        2 => format!("2/{n}"),
        3 => format!("3/{}/{n}", &n[..1]),
        _ => format!("{}/{}/{n}", &n[..2], &n[2..4]),
    };
    let url = format!("https://index.crates.io/{path}");
    let out = Command::new("curl")
        .args(["-sSf", "--max-time", "30", &url])
        .output()
        .context("could not run curl")?;
    if !out.status.success() {
        bail!(
            "fetching {url} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `pubtime` of `version` in an index file (one JSON object per line). A tiny scan rather
/// than a JSON dependency: the fields are flat strings.
fn pubtime_of(index: &str, version: &str) -> Option<String> {
    let needle = format!("\"vers\":\"{version}\"");
    let line = index.lines().find(|l| l.contains(&needle))?;
    let start = line.find("\"pubtime\":\"")? + "\"pubtime\":\"".len();
    let rest = &line[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

/// Today's date (UTC) as `YYYY-MM-DD`, from `date` to avoid a time dependency.
fn today() -> Result<String> {
    let out = Command::new("date")
        .args(["-u", "+%Y-%m-%d"])
        .output()
        .context("could not run date")?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn shift_days(date: &str, days: i64) -> Result<String> {
    let out = Command::new("date")
        .args(["-u", "-d", &format!("{date} {days} days"), "+%Y-%m-%d"])
        .output()
        .context("could not run date")?;
    if !out.status.success() {
        bail!("date arithmetic failed");
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_registry_packages_only() {
        let lock = r#"
[[package]]
name = "a"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "local"
version = "0.1.0"
"#;
        let p = parse_lock(lock);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].name, "a");
    }

    #[test]
    fn finds_pubtime() {
        let idx = r#"{"name":"a","vers":"1.0.0","pubtime":"2026-01-02T03:04:05Z"}
{"name":"a","vers":"1.0.1","pubtime":"2026-02-02T03:04:05Z"}"#;
        assert_eq!(
            pubtime_of(idx, "1.0.1").as_deref(),
            Some("2026-02-02T03:04:05Z")
        );
        assert_eq!(pubtime_of(idx, "9.9.9"), None);
    }

    #[test]
    fn parses_exceptions() {
        let t = "[[exception]]\ncrate = \"tokio\"\nversion = \"1.54.1\"\nreason = \"fix\"\nexpires = \"2026-11-04\"\n";
        let e = parse_exceptions(t);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].krate, "tokio");
        assert_eq!(e[0].expires, "2026-11-04");
    }
}
