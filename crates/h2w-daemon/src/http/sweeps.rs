//! Sweep lists for the experiment views, one sweep at a time and cached.
//!
//! Listing an experiment's sweeps reads every task's `summary.json` and the sweep's
//! `judged.json`, and `GET /experiments` does that for every experiment. Each sweep's
//! summary is therefore cached under a stamp of the files it was computed from: the sweep
//! directory, each task's `summary.json` and the judged files (size, modification time,
//! inode). A sweep is read again only when its stamp changed (or after [`MAX_AGE`], which
//! bounds what a change within one timestamp tick could hide). A sweep that cannot be read
//! (`CoreError::TooLarge`, ...) is left out of the list with one log line per change, and the
//! other sweeps of the experiment are listed as usual.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use h2w_api::{ExperimentKind, SweepSummary};
use h2w_core::namespace::Namespace;
use h2w_core::results::ExperimentDir;

/// Longest a cached sweep is used without reading it again.
pub const MAX_AGE: Duration = Duration::from_secs(300);
/// Most sweeps cached (over all experiments); the cache starts over beyond it.
const MAX_ENTRIES: usize = 4096;
/// Most entries of one sweep directory stamped; a larger one is read every time (the core
/// reader refuses more than 10,000 task directories anyway).
const MAX_STAMP_ENTRIES: usize = 10_100;

struct Entry {
    stamp: u64,
    at: Instant,
    /// The sweep, or why it could not be read.
    summary: Result<SweepSummary, String>,
}

/// Cached sweep summaries by experiment and sweep id.
#[derive(Default)]
pub struct SweepCache {
    entries: Mutex<HashMap<(String, String), Entry>>,
}

impl std::fmt::Debug for SweepCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SweepCache")
    }
}

impl SweepCache {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<(String, String), Entry>> {
        match self.entries.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    fn cached(&self, key: &(String, String), stamp: u64) -> Option<Result<SweepSummary, String>> {
        let map = self.lock();
        let e = map.get(key)?;
        (e.stamp == stamp && e.at.elapsed() < MAX_AGE).then(|| e.summary.clone())
    }

    fn store(&self, key: (String, String), stamp: u64, summary: Result<SweepSummary, String>) {
        let mut map = self.lock();
        if map.len() >= MAX_ENTRIES && !map.contains_key(&key) {
            map.clear();
        }
        map.insert(
            key,
            Entry {
                stamp,
                at: Instant::now(),
                summary,
            },
        );
    }

    /// The sweeps of `experiment` (its directory `path`, opened as `dir`), newest first,
    /// without the ones that cannot be read. Blocking: call from a blocking task.
    pub fn list(
        &self,
        experiment: &str,
        path: &Path,
        kind: ExperimentKind,
        dir: &ExperimentDir,
    ) -> Vec<SweepSummary> {
        let names = match dir.sweep_namespaces() {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(experiment = %experiment, error = %e, "could not list the sweeps of an experiment");
                return Vec::new();
            }
        };
        let mut out = Vec::with_capacity(names.len());
        let mut present = HashSet::with_capacity(names.len());
        for ns in &names {
            let id = ns.to_string();
            present.insert(id.clone());
            let key = (experiment.to_owned(), id);
            let stamp = sweep_stamp(path, kind, ns);
            let summary = match stamp.and_then(|s| self.cached(&key, s)) {
                Some(hit) => hit,
                None => {
                    let read = dir.sweep(ns).map_err(|e| e.to_string());
                    if let Err(e) = &read {
                        tracing::warn!(
                            experiment = %experiment,
                            sweep = %key.1,
                            error = %e,
                            "a sweep could not be read; it is left out of the experiment's list"
                        );
                    }
                    if let Some(s) = stamp {
                        self.store(key, s, read.clone());
                    }
                    read
                }
            };
            if let Ok(s) = summary {
                out.push(s);
            }
        }
        // Forget sweeps that are gone.
        self.lock()
            .retain(|(exp, id), _| exp != experiment || present.contains(id));
        out.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        out
    }
}

fn hash_meta(h: &mut impl Hasher, m: &std::fs::Metadata) {
    (m.len(), m.mtime(), m.mtime_nsec(), m.ino(), m.dev()).hash(h);
}

/// A stamp of what a sweep's summary is computed from, or `None` when it cannot be taken
/// (then the sweep is read every time). Symlinks are never followed.
fn sweep_stamp(exp: &Path, kind: ExperimentKind, ns: &Namespace) -> Option<u64> {
    let root = h2w_core::namespace::namespaces_root(kind, &ns.bench_name)?;
    let dir = exp.join(root).join(ns.summary().to_string());
    let meta = std::fs::symlink_metadata(&dir).ok()?;
    if !meta.is_dir() {
        return None;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    hash_meta(&mut h, &meta);
    let mut parts: Vec<(String, Option<std::fs::Metadata>)> = Vec::new();
    for (i, entry) in std::fs::read_dir(&dir).ok()?.enumerate() {
        if i >= MAX_STAMP_ENTRIES {
            return None;
        }
        let entry = entry.ok()?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let ft = entry.file_type().ok()?;
        if ft.is_dir() {
            let summary = std::fs::symlink_metadata(entry.path().join("summary.json")).ok();
            parts.push((name, summary));
        } else if ft.is_file() && matches!(name.as_str(), "judged.json" | "base_judged.json") {
            parts.push((name, entry.metadata().ok()));
        }
    }
    parts.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, m) in &parts {
        name.hash(&mut h);
        match m {
            Some(m) => hash_meta(&mut h, m),
            None => 0_u8.hash(&mut h),
        }
    }
    Some(h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(dir: &Path, task: &str, status: &str) {
        let p = dir.join(task);
        std::fs::create_dir_all(&p).unwrap();
        let body = serde_json::json!({
            "task_id": task, "domain": "d", "status": status,
            "modes": {"agg": {"ok": true, "stages": [{"usd": 0.5}]}},
        });
        std::fs::write(p.join("summary.json"), body.to_string()).unwrap();
    }

    #[test]
    fn stamps_follow_the_files_a_summary_is_read_from() {
        let root = tempfile::tempdir().unwrap();
        let sweep = root.path().join("lab/adapt/lab-oc-agg1");
        summary(&sweep, "t1", "ok");
        let ns = Namespace::parse("lab-oc-agg1").unwrap();
        let stamp = || sweep_stamp(root.path(), ExperimentKind::Standard, &ns).unwrap();
        let first = stamp();
        assert_eq!(stamp(), first);
        // A rewritten summary (a new inode, as harness2's write-and-rename gives), a new
        // task and a judged file each change it.
        summary(&sweep, "t1", "partial");
        let after_rewrite = stamp();
        assert_ne!(after_rewrite, first);
        summary(&sweep, "t2", "ok");
        let after_task = stamp();
        assert_ne!(after_task, after_rewrite);
        std::fs::write(sweep.join("judged.json"), "[]").unwrap();
        assert_ne!(stamp(), after_task);
        // Any new entry changes the directory itself; a missing sweep has no stamp.
        let before = stamp();
        std::fs::write(sweep.join("notes.txt"), "x").unwrap();
        assert_ne!(stamp(), before);
        let gone = Namespace::parse("lab-oc-agg2").unwrap();
        assert!(sweep_stamp(root.path(), ExperimentKind::Standard, &gone).is_none());
    }

    #[test]
    fn cached_sweeps_are_reused_until_their_files_change() {
        let root = tempfile::tempdir().unwrap();
        let sweep = root.path().join("lab/adapt/lab-oc-agg1");
        summary(&sweep, "t1", "ok");
        let dir = ExperimentDir::open(root.path(), ExperimentKind::Standard, &[]).unwrap();
        let cache = SweepCache::default();
        let list = |c: &SweepCache| c.list("e1", root.path(), ExperimentKind::Standard, &dir);
        let first = list(&cache);
        assert_eq!((first.len(), first[0].ok), (1, 1));
        assert_eq!(cache.lock().len(), 1);
        // Same files: served from the cache (an entry that would read differently is
        // returned as stored).
        {
            let mut map = cache.lock();
            let e = map.values_mut().next().unwrap();
            if let Ok(s) = e.summary.as_mut() {
                s.ok = 99;
            }
        }
        assert_eq!(list(&cache)[0].ok, 99);
        // A changed summary is read again.
        summary(&sweep, "t2", "ok");
        let again = list(&cache);
        assert_eq!((again[0].tasks, again[0].ok), (2, 2));
        // A sweep that disappears is forgotten.
        std::fs::remove_dir_all(&sweep).unwrap();
        assert!(list(&cache).is_empty());
        assert!(cache.lock().is_empty());
    }
}
