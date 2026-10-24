//! Sweep lists for the experiment views and estimates: bounded, one sweep at a time, cached.
//!
//! Listing an experiment's sweeps reads every task's `summary.json` and the sweep's judged
//! files, and `GET /experiments` does that for every experiment. So:
//! - at most [`MAX_SWEEPS_LISTED`] sweeps of one experiment are listed: the newest by the
//!   modification time of their directories. The others stay reachable by id through the
//!   results routes; a warning names the experiment once per count;
//! - each sweep's summary is cached under a stamp of the files it was computed from (the
//!   sweep directory, each task's `summary.json` and the judged files: size, modification
//!   time, inode). A changed or older-than-[`MAX_AGE`] entry is served as it is while one
//!   background task, under a read permit that is free at that moment, reads the sweep again
//!   (stale-while-revalidate), so a list call never waits for a slow sweep it has seen
//!   before. At most [`MAX_REFRESHING`] refreshes run at once; a sweep without an entry is
//!   read in the request;
//! - the cache holds [`MAX_ENTRIES`] sweeps and evicts the least recently used;
//! - misses are read within the request's [`ReadBudget`]; when it runs out (or the request
//!   is gone) the listing stops there, without caching anything for the sweeps not read: the
//!   next request goes on from the cache;
//! - a sweep that cannot be read (`CoreError::TooLarge` for a file over its cap, ...) is
//!   left out of the list with one warning per change; the other sweeps are listed as usual.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use h2w_api::{ExperimentKind, SweepSummary};
use h2w_core::namespace::Namespace;
use h2w_core::results::{ExperimentDir, ReadBudget};

use crate::reads::{ReadCancel, Reads};

/// Longest a cached sweep is served before it is read again (in the background).
pub const MAX_AGE: Duration = Duration::from_secs(300);
/// Most sweeps cached over all experiments; the least recently used go first.
pub const MAX_ENTRIES: usize = 4096;
/// Most sweeps listed per experiment (the newest by directory modification time).
pub const MAX_SWEEPS_LISTED: usize = 200;
/// Background refreshes running at once.
pub const MAX_REFRESHING: usize = 16;
/// Most entries of one sweep directory stamped; a larger one is read every time (the core
/// reader refuses more than 10,000 task directories anyway).
const MAX_STAMP_ENTRIES: usize = 10_100;

type Key = (String, String);

struct Entry {
    stamp: u64,
    /// When the summary was read.
    at: Instant,
    /// When it was last served.
    used: Instant,
    /// The sweep, or why it could not be read.
    summary: Result<SweepSummary, String>,
}

/// What the cache holds for a sweep.
enum Lookup {
    Fresh(Result<SweepSummary, String>),
    Stale(Result<SweepSummary, String>),
    Missing,
}

/// Cached sweep summaries by experiment and sweep id.
pub struct SweepCache {
    entries: Mutex<HashMap<Key, Entry>>,
    refreshing: Mutex<HashSet<Key>>,
    /// Sweep counts already warned about, by experiment.
    warned: Mutex<HashMap<String, usize>>,
    max_entries: usize,
    max_listed: usize,
}

impl Default for SweepCache {
    fn default() -> Self {
        Self::with_limits(MAX_ENTRIES, MAX_SWEEPS_LISTED)
    }
}

impl std::fmt::Debug for SweepCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SweepCache")
    }
}

fn locked<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

impl SweepCache {
    /// A cache of `max_entries` sweeps listing at most `max_listed` per experiment.
    #[must_use]
    pub fn with_limits(max_entries: usize, max_listed: usize) -> SweepCache {
        SweepCache {
            entries: Mutex::new(HashMap::new()),
            refreshing: Mutex::new(HashSet::new()),
            warned: Mutex::new(HashMap::new()),
            max_entries: max_entries.max(1),
            max_listed: max_listed.max(1),
        }
    }

    fn lookup(&self, key: &Key, stamp: Option<u64>) -> Lookup {
        let mut map = locked(&self.entries);
        let Some(e) = map.get_mut(key) else {
            return Lookup::Missing;
        };
        let Some(stamp) = stamp else {
            // Without a stamp nothing tells whether the entry still holds.
            return Lookup::Missing;
        };
        e.used = Instant::now();
        if e.stamp == stamp && e.at.elapsed() < MAX_AGE {
            Lookup::Fresh(e.summary.clone())
        } else {
            Lookup::Stale(e.summary.clone())
        }
    }

    fn store(&self, key: Key, stamp: u64, summary: Result<SweepSummary, String>) {
        let mut map = locked(&self.entries);
        if map.len() >= self.max_entries && !map.contains_key(&key) {
            let oldest = map
                .iter()
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                map.remove(&k);
            }
        }
        let now = Instant::now();
        map.insert(
            key,
            Entry {
                stamp,
                at: now,
                used: now,
                summary,
            },
        );
    }

    /// Read one sweep within `budget`. `Err(None)` when the budget ran out or the request
    /// is gone (nothing to cache); `Err(Some(reason))` when the sweep itself cannot be read.
    fn read(
        dir: &ExperimentDir,
        key: &Key,
        ns: &Namespace,
        budget: &mut ReadBudget,
    ) -> Result<SweepSummary, Option<String>> {
        match dir.sweep_with(ns, budget) {
            Ok(s) => Ok(s),
            Err(e) if crate::reads::spent(&e) => {
                tracing::debug!(experiment = %key.0, sweep = %key.1, error = %e, "sweep not read: the request's read budget is spent");
                Err(None)
            }
            Err(e) => {
                tracing::warn!(
                    experiment = %key.0,
                    sweep = %key.1,
                    error = %e,
                    "a sweep could not be read; it is left out of the experiment's list"
                );
                Err(Some(e.to_string()))
            }
        }
    }

    /// Read `ns` again in the background, if no refresh of it runs, fewer than
    /// [`MAX_REFRESHING`] do, and a read permit is free now. Returns whether one started.
    fn refresh(
        &self,
        reads: &Arc<Reads>,
        key: &Key,
        path: &Path,
        kind: ExperimentKind,
        ns: &Namespace,
    ) -> bool {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return false;
        };
        let mut running = locked(&self.refreshing);
        if running.contains(key) || running.len() >= MAX_REFRESHING {
            return false;
        }
        let Some(permit) = reads.try_permit() else {
            return false;
        };
        running.insert(key.clone());
        let reads = Arc::clone(reads);
        let key = key.clone();
        let path = path.to_path_buf();
        let ns = ns.clone();
        handle.spawn_blocking(move || {
            let _permit = permit;
            let cache = &reads.sweeps;
            if let Some(stamp) = sweep_stamp(&path, kind, &ns)
                && let Ok(dir) = crate::reads::open_experiment(&path, kind)
            {
                let mut budget = ReadBudget::new(crate::reads::SWEEP_BYTES, 1);
                match Self::read(&dir, &key, &ns, &mut budget) {
                    Ok(s) => cache.store(key.clone(), stamp, Ok(s)),
                    Err(Some(why)) => cache.store(key.clone(), stamp, Err(why)),
                    Err(None) => {}
                }
            }
            locked(&cache.refreshing).remove(&key);
        });
        true
    }

    /// At most `max_listed` of `names`: the newest by directory modification time.
    fn newest(
        &self,
        experiment: &str,
        path: &Path,
        kind: ExperimentKind,
        names: Vec<Namespace>,
    ) -> Vec<Namespace> {
        if names.len() <= self.max_listed {
            return names;
        }
        let total = names.len();
        let mut timed: Vec<(Option<SystemTime>, Namespace)> = names
            .into_iter()
            .map(|ns| (sweep_dir(path, kind, &ns).and_then(|d| modified(&d)), ns))
            .collect();
        timed.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.1.to_string().cmp(&b.1.to_string()))
        });
        timed.truncate(self.max_listed);
        let mut warned = locked(&self.warned);
        if warned.get(experiment) != Some(&total) {
            warned.insert(experiment.to_owned(), total);
            tracing::warn!(
                experiment = %experiment,
                sweeps = total,
                listed = self.max_listed,
                "the experiment has more sweeps than are listed; the newest are listed and the others stay reachable by id"
            );
        }
        timed.into_iter().map(|(_, ns)| ns).collect()
    }

    /// The sweeps of `experiment` (its directory `path`, opened as `dir`), newest first,
    /// without the ones that cannot be read; misses are read within `budget`. Blocking: call
    /// from a blocking task that holds a read permit.
    #[allow(clippy::too_many_arguments)] // the request's context, passed through
    pub fn list(
        &self,
        reads: &Arc<Reads>,
        experiment: &str,
        path: &Path,
        kind: ExperimentKind,
        dir: &ExperimentDir,
        cancel: &ReadCancel,
        budget: &mut ReadBudget,
    ) -> Vec<SweepSummary> {
        let names = match dir.sweep_namespaces() {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(experiment = %experiment, error = %e, "could not list the sweeps of an experiment");
                return Vec::new();
            }
        };
        let names = self.newest(experiment, path, kind, names);
        let mut out = Vec::with_capacity(names.len());
        let mut present = HashSet::with_capacity(names.len());
        let mut complete = true;
        for ns in &names {
            if cancel.is_cancelled() {
                complete = false;
                break;
            }
            let key = (experiment.to_owned(), ns.to_string());
            present.insert(key.1.clone());
            let stamp = sweep_stamp(path, kind, ns);
            let summary = match self.lookup(&key, stamp) {
                Lookup::Fresh(s) => s,
                // Served as it is and read again in the background; read here only when
                // there is no runtime to do it (unit tests).
                Lookup::Stale(s) if tokio::runtime::Handle::try_current().is_ok() => {
                    self.refresh(reads, &key, path, kind, ns);
                    s
                }
                Lookup::Stale(_) | Lookup::Missing => match Self::read(dir, &key, ns, budget) {
                    Ok(s) => {
                        if let Some(st) = stamp {
                            self.store(key, st, Ok(s.clone()));
                        }
                        Ok(s)
                    }
                    Err(Some(why)) => {
                        if let Some(st) = stamp {
                            self.store(key, st, Err(why.clone()));
                        }
                        Err(why)
                    }
                    // The request's budget is spent: list what was read (the next request
                    // goes on from the cache).
                    Err(None) => {
                        complete = false;
                        break;
                    }
                },
            };
            if let Ok(s) = summary {
                out.push(s);
            }
        }
        // Forget sweeps that are gone (only after a complete pass).
        if complete {
            locked(&self.entries).retain(|(exp, id), _| exp != experiment || present.contains(id));
        }
        out.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        out
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        locked(&self.entries).len()
    }
}

fn hash_meta(h: &mut impl Hasher, m: &std::fs::Metadata) {
    (m.len(), m.mtime(), m.mtime_nsec(), m.ino(), m.dev()).hash(h);
}

/// The directory a sweep's summaries live in.
fn sweep_dir(exp: &Path, kind: ExperimentKind, ns: &Namespace) -> Option<PathBuf> {
    let root = h2w_core::namespace::namespaces_root(kind, &ns.bench_name)?;
    Some(exp.join(root).join(ns.summary().to_string()))
}

/// Modification time of a directory (never following a symlink).
fn modified(p: &Path) -> Option<SystemTime> {
    let m = std::fs::symlink_metadata(p).ok()?;
    m.is_dir().then(|| m.modified().ok()).flatten()
}

/// A stamp of what a sweep's summary is computed from, or `None` when it cannot be taken
/// (then the sweep is read every time). Symlinks are never followed.
fn sweep_stamp(exp: &Path, kind: ExperimentKind, ns: &Namespace) -> Option<u64> {
    let dir = sweep_dir(exp, kind, ns)?;
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

    fn open(root: &Path) -> ExperimentDir {
        ExperimentDir::open(root, ExperimentKind::Standard, &[]).unwrap()
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

    /// Without a runtime (no background refresh) a changed sweep is read in the call.
    #[test]
    fn cached_sweeps_are_reused_until_their_files_change() {
        let root = tempfile::tempdir().unwrap();
        let sweep = root.path().join("lab/adapt/lab-oc-agg1");
        summary(&sweep, "t1", "ok");
        let dir = open(root.path());
        let reads = Reads::new();
        let cancel = ReadCancel::default();
        let list = || {
            reads.sweeps.list(
                &reads,
                "e1",
                root.path(),
                ExperimentKind::Standard,
                &dir,
                &cancel,
                &mut ReadBudget::unlimited(),
            )
        };
        let first = list();
        assert_eq!((first.len(), first[0].ok), (1, 1));
        assert_eq!(reads.sweeps.len(), 1);
        // Same files: served from the cache (an entry that would read differently is
        // returned as stored).
        {
            let mut map = locked(&reads.sweeps.entries);
            let e = map.values_mut().next().unwrap();
            if let Ok(s) = e.summary.as_mut() {
                s.ok = 99;
            }
        }
        assert_eq!(list()[0].ok, 99);
        summary(&sweep, "t2", "ok");
        let again = list();
        assert_eq!((again[0].tasks, again[0].ok), (2, 2));
        // A sweep that disappears is forgotten.
        std::fs::remove_dir_all(&sweep).unwrap();
        assert!(list().is_empty());
        assert_eq!(reads.sweeps.len(), 0);
    }

    /// With a runtime, a changed sweep is served as it was while it is read again in the
    /// background, and a later list shows the new summary.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn changed_sweeps_are_served_stale_while_they_are_read_again() {
        let root = tempfile::tempdir().unwrap();
        let sweep = root.path().join("lab/adapt/lab-oc-agg1");
        summary(&sweep, "t1", "ok");
        let reads = Reads::new();
        let path = root.path().to_path_buf();
        let run = |reads: Arc<Reads>, path: PathBuf| {
            tokio::task::spawn_blocking(move || {
                let dir = open(&path);
                reads.sweeps.list(
                    &reads,
                    "e1",
                    &path,
                    ExperimentKind::Standard,
                    &dir,
                    &ReadCancel::default(),
                    &mut ReadBudget::unlimited(),
                )
            })
        };
        let first = run(Arc::clone(&reads), path.clone()).await.unwrap();
        assert_eq!(first[0].tasks, 1);
        summary(&sweep, "t2", "ok");
        let stale = run(Arc::clone(&reads), path.clone()).await.unwrap();
        assert_eq!(stale[0].tasks, 1, "served as it was");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let now = run(Arc::clone(&reads), path.clone()).await.unwrap();
            if now[0].tasks == 2 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the background refresh never landed"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while !locked(&reads.sweeps.refreshing).is_empty() {
            assert!(Instant::now() < deadline, "a refresh never finished");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            reads.permits.available_permits(),
            crate::reads::RESULTS_READS
        );
    }

    #[test]
    fn the_least_recently_used_sweep_is_evicted() {
        let cache = SweepCache::with_limits(2, 10);
        let summary = |id: &str| -> Result<SweepSummary, String> { Err(id.to_owned()) };
        let k = |id: &str| ("e".to_owned(), id.to_owned());
        cache.store(k("a"), 1, summary("a"));
        cache.store(k("b"), 1, summary("b"));
        // `a` is used again, so `b` is the least recently used.
        assert!(matches!(cache.lookup(&k("a"), Some(1)), Lookup::Fresh(_)));
        cache.store(k("c"), 1, summary("c"));
        assert_eq!(cache.len(), 2);
        assert!(matches!(cache.lookup(&k("b"), Some(1)), Lookup::Missing));
        assert!(matches!(cache.lookup(&k("a"), Some(1)), Lookup::Fresh(_)));
        assert!(matches!(cache.lookup(&k("a"), Some(2)), Lookup::Stale(_)));
    }

    /// Past the per-experiment cap only the newest sweeps (by directory time) are listed.
    #[test]
    fn at_most_the_newest_sweeps_are_listed() {
        let root = tempfile::tempdir().unwrap();
        for (i, name) in ["lab-oc-agg1", "lab-oc-agg2", "lab-oc-agg3"]
            .iter()
            .enumerate()
        {
            let sweep = root.path().join("lab/adapt").join(name);
            summary(&sweep, "t1", "ok");
            let secs = 1_700_000_000 + 100 * u64::try_from(i).unwrap();
            let t = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
            std::fs::File::open(&sweep)
                .unwrap()
                .set_modified(t)
                .unwrap();
        }
        let dir = open(root.path());
        let reads = Arc::new(Reads {
            permits: Arc::new(tokio::sync::Semaphore::new(crate::reads::RESULTS_READS)),
            sweeps: SweepCache::with_limits(10, 2),
        });
        let listed = reads.sweeps.list(
            &reads,
            "e1",
            root.path(),
            ExperimentKind::Standard,
            &dir,
            &ReadCancel::default(),
            &mut ReadBudget::unlimited(),
        );
        let mut ids: Vec<&str> = listed.iter().map(|s| s.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["lab-oc-agg2", "lab-oc-agg3"]);
        // A cancelled request stops listing.
        let cancel = ReadCancel::default();
        cancel.cancel();
        let none = reads.sweeps.list(
            &reads,
            "e2",
            root.path(),
            ExperimentKind::Standard,
            &dir,
            &cancel,
            &mut ReadBudget::unlimited(),
        );
        assert!(none.is_empty());
    }

    /// A spent budget stops the listing without caching anything for the sweeps it did not
    /// read; the next request, with a fresh budget, lists them.
    #[test]
    fn a_spent_budget_stops_the_listing_without_caching_failures() {
        let root = tempfile::tempdir().unwrap();
        for name in ["lab-oc-agg1", "lab-oc-agg2", "lab-oc-agg3"] {
            summary(&root.path().join("lab/adapt").join(name), "t1", "ok");
        }
        let dir = open(root.path());
        let reads = Reads::new();
        let cancel = ReadCancel::default();
        let list = |budget: &mut ReadBudget| {
            reads.sweeps.list(
                &reads,
                "e1",
                root.path(),
                ExperimentKind::Standard,
                &dir,
                &cancel,
                budget,
            )
        };
        let first = list(&mut cancel.budget(1 << 20, 2));
        assert_eq!(first.len(), 2);
        assert_eq!(
            reads.sweeps.len(),
            2,
            "the unread sweep is not cached as failed"
        );
        // Cached sweeps cost nothing: one more sweep of budget lists all three.
        let all = list(&mut cancel.budget(1 << 20, 1));
        assert_eq!(all.len(), 3);
    }
}
