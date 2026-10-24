//! Process identity from `/proc` (Linux; harness2 is Linux-only) and process-group signals.
//!
//! A pid alone is not an identity: after a restart the number may belong to another
//! process. Jobs record the leader's pid, its start time (clock ticks since boot,
//! `/proc/<pid>/stat` field 22) and the boot id (`/proc/sys/kernel/random/boot_id`), and
//! every step process gets [`JOB_MARKER_ENV`] = the job id in its environment. Recovery
//! signals a recorded group only on the same boot and only when a live member of the group
//! still carries that marker in `/proc/<pid>/environ` (positive identification); nothing is
//! signalled on a guess.

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;

/// The fields of `/proc/<pid>/stat` this module needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcStat {
    pub state: char,
    pub pgrp: i32,
    pub start: u64,
}

/// Parse the text of `/proc/<pid>/stat`. The command name (field 2) may contain spaces and
/// parentheses, so parsing starts after the last `)`.
#[must_use]
pub fn parse_stat(text: &str) -> Option<ProcStat> {
    let after = &text[text.rfind(')')? + 1..];
    let fields: Vec<&str> = after.split_whitespace().collect();
    // fields[0] is field 3 (state); field N is fields[N - 3].
    let state = fields.first()?.chars().next()?;
    let pgrp = fields.get(2)?.parse().ok()?;
    let start = fields.get(19)?.parse().ok()?;
    Some(ProcStat { state, pgrp, start })
}

/// `/proc/<pid>/stat` of a live process.
#[must_use]
pub fn stat(pid: i32) -> Option<ProcStat> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_stat(&text)
}

/// Start time of a process, if it exists.
#[must_use]
pub fn start_time(pid: i32) -> Option<u64> {
    stat(pid).map(|s| s.start)
}

/// `true` when `pid` exists, is not a zombie, and started at `start`.
#[must_use]
pub fn is_same_process(pid: i32, start: u64) -> bool {
    stat(pid).is_some_and(|s| s.start == start && s.state != 'Z')
}

/// Environment variable every step process gets: the id of its job.
pub const JOB_MARKER_ENV: &str = "H2W_JOB_ID";

/// Largest `/proc/<pid>/environ` read when looking for the marker.
const MAX_ENVIRON: u64 = 4 * 1024 * 1024;

/// Reads of an environment that comes back empty or denied before the process counts as
/// unmarked; the pause before each new read doubles from 1 ms ([`environ_pause`], about 0.25 s
/// in all). Both happen for a moment during an exec: the new program's name is set before its
/// environment (`/proc/<pid>/environ` reads empty, under 0.2 ms when measured on Linux 7.0),
/// and a child of a non-dumpable process is unreadable until the exec makes it dumpable.
const ENVIRON_READS: u32 = 9;

fn environ_pause(retry: u32) -> std::time::Duration {
    std::time::Duration::from_millis(1 << retry)
}

/// The id of the running boot (`/proc/sys/kernel/random/boot_id`).
#[must_use]
pub fn boot_id() -> Option<String> {
    let s = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    let s = s.trim();
    let ok =
        !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-');
    ok.then(|| s.to_owned())
}

/// `true` when the environment of `pid` holds `H2W_JOB_ID=<job_id>` (readable only for
/// processes of the same user that are dumpable, as job processes are). An environment that
/// reads empty or denied is read again ([`ENVIRON_READS`]) before the process counts as
/// unmarked.
#[must_use]
pub fn carries_marker(pid: i32, job_id: &str) -> bool {
    let mut state = marker_state(pid, job_id);
    for retry in 0..ENVIRON_READS - 1 {
        if state.is_some() {
            break;
        }
        std::thread::sleep(environ_pause(retry));
        state = marker_state(pid, job_id);
    }
    state == Some(true)
}

/// What one read of the environment of `pid` says about the marker of `job_id`; `None` when
/// it reads empty or is denied (mid-exec; also an exiting process, an empty environment or
/// another user's process, which stay unmarked after the last read).
fn marker_state(pid: i32, job_id: &str) -> Option<bool> {
    use std::io::Read;
    let f = match std::fs::File::open(format!("/proc/{pid}/environ")) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return None,
        Err(_) => return Some(false),
    };
    let mut buf = Vec::new();
    if f.take(MAX_ENVIRON).read_to_end(&mut buf).is_err() {
        return Some(false);
    }
    if buf.is_empty() {
        return None;
    }
    let want = format!("{JOB_MARKER_ENV}={job_id}");
    Some(buf.split(|b| *b == 0).any(|e| e == want.as_bytes()))
}

/// What was recorded about a step's leader when it started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub pid: i32,
    pub start: u64,
    pub boot_id: String,
    pub job_id: String,
}

/// The outcome of [`terminate_recorded_group`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Termination {
    /// Members were identified and signalled.
    Signalled,
    /// Recorded on another boot: the pid identifies nothing now.
    OtherBoot,
    /// No live member of the group carries the job's marker.
    NotFound,
}

/// Live members of the recorded group that carry the job's marker. The group id is the
/// leader's pid; members started no earlier than the leader (a necessary condition) and
/// the marker is the identification. Like [`carries_marker`], without blocking the runtime.
async fn marked_members(rec: &Recorded) -> Vec<i32> {
    let mut marked = Vec::new();
    for pid in group_members(rec.pid, rec.start) {
        let mut state = marker_state(pid, &rec.job_id);
        for retry in 0..ENVIRON_READS - 1 {
            if state.is_some() {
                break;
            }
            tokio::time::sleep(environ_pause(retry)).await;
            state = marker_state(pid, &rec.job_id);
        }
        if state == Some(true) {
            marked.push(pid);
        }
    }
    marked
}

/// Live (non-zombie) members of process group `pgid` that started at or after `min_start`.
#[must_use]
pub fn group_members(pgid: i32, min_start: u64) -> Vec<i32> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    dir.filter_map(Result::ok)
        .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
        .filter(|pid| {
            stat(*pid).is_some_and(|s| s.pgrp == pgid && s.start >= min_start && s.state != 'Z')
        })
        .collect()
}

/// Send `sig` to process group `pgid`. Refuses group ids that would hit this server's own
/// group or every process (`0`, `1`, negatives).
///
/// # Errors
/// `ESRCH` when the group is gone; `EPERM`; or `EINVAL` for a refused id.
pub fn signal_group(pgid: i32, sig: Signal) -> nix::Result<()> {
    if pgid <= 1 || Pid::from_raw(pgid) == nix::unistd::getpgrp() {
        return Err(nix::errno::Errno::EINVAL);
    }
    killpg(Pid::from_raw(pgid), sig)
}

/// `true` while any member of the group is alive.
#[must_use]
pub fn group_alive(pgid: i32) -> bool {
    if pgid <= 1 {
        return false;
    }
    // Signal 0 checks existence and permission without delivering anything.
    match killpg(Pid::from_raw(pgid), None) {
        Ok(()) => !group_members(pgid, 0).is_empty(),
        Err(_) => false,
    }
}

/// Terminate a job's process group found after a restart (or left by a runner that
/// panicked): only on the recorded boot and only while a member of the group carries the
/// job's marker. SIGTERM, then SIGKILL after `grace` if a marked member is still alive (so
/// the group id is still the job's).
pub async fn terminate_recorded_group(rec: &Recorded, grace: std::time::Duration) -> Termination {
    if boot_id().as_deref() != Some(rec.boot_id.as_str()) {
        return Termination::OtherBoot;
    }
    if marked_members(rec).await.is_empty() || signal_group(rec.pid, Signal::SIGTERM).is_err() {
        return Termination::NotFound;
    }
    let deadline = tokio::time::Instant::now() + grace;
    while tokio::time::Instant::now() < deadline {
        if marked_members(rec).await.is_empty() {
            return Termination::Signalled;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    if !marked_members(rec).await.is_empty() {
        let _ = signal_group(rec.pid, Signal::SIGKILL);
    }
    Termination::Signalled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names_with_spaces_and_parens() {
        let text = "1234 (my (odd) name) S 1 1234 1234 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 98765 1000 10 18446744073709551615";
        let s = parse_stat(text).unwrap();
        assert_eq!(s.state, 'S');
        assert_eq!(s.pgrp, 1234);
        assert_eq!(s.start, 98765);
        assert!(parse_stat("garbage").is_none());
        assert!(parse_stat("1 (x) S 1").is_none());
    }

    #[test]
    fn reads_this_process() {
        let me = i32::try_from(std::process::id()).unwrap();
        let s = stat(me).unwrap();
        assert!(s.start > 0);
        assert!(is_same_process(me, s.start));
        assert!(!is_same_process(me, s.start + 1));
    }

    #[test]
    fn refuses_dangerous_group_ids() {
        for pg in [-1, 0, 1] {
            assert_eq!(
                signal_group(pg, Signal::SIGTERM),
                Err(nix::errno::Errno::EINVAL)
            );
        }
        let own = nix::unistd::getpgrp().as_raw();
        assert_eq!(
            signal_group(own, Signal::SIGTERM),
            Err(nix::errno::Errno::EINVAL)
        );
    }

    /// Spawn `sh -c script` as its own process group, with or without the marker.
    fn spawn_group(script: &str, marker: Option<&str>) -> (std::process::Child, Recorded) {
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", script]).env_remove(JOB_MARKER_ENV);
        if let Some(m) = marker {
            cmd.env(JOB_MARKER_ENV, m);
        }
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        let child = cmd.spawn().unwrap();
        let pid = i32::try_from(child.id()).unwrap();
        let rec = Recorded {
            pid,
            start: start_time(pid).unwrap(),
            boot_id: boot_id().unwrap(),
            job_id: "job-1".into(),
        };
        (child, rec)
    }

    /// An exec names the new program before its environment is in place, and for that
    /// moment `/proc/<pid>/environ` reads empty: the marker check must look again, not
    /// answer "unmarked". (Until the new name is set, a child started by `spawn` may still
    /// share this process's memory, so the test waits for it first.)
    #[test]
    fn a_process_caught_mid_exec_still_carries_its_marker() {
        for _ in 0..50 {
            let mut child = std::process::Command::new("sleep")
                .arg("30")
                .env(JOB_MARKER_ENV, "job-1")
                .spawn()
                .unwrap();
            let pid = i32::try_from(child.id()).unwrap();
            while std::fs::read_to_string(format!("/proc/{pid}/comm"))
                .is_ok_and(|c| c.trim() != "sleep")
            {
                std::hint::spin_loop();
            }
            let marked = carries_marker(pid, "job-1");
            let _ = child.kill();
            let _ = child.wait();
            assert!(marked, "a process just started lost its marker");
        }
    }

    #[test]
    fn the_boot_id_is_read() {
        let a = boot_id().unwrap();
        assert_eq!(a.len(), 36, "{a}");
        assert_eq!(boot_id().unwrap(), a);
    }

    #[tokio::test]
    async fn terminates_a_marked_group() {
        let (mut child, rec) = spawn_group("exec sleep 30", Some("job-1"));
        assert!(group_alive(rec.pid));
        assert!(carries_marker(rec.pid, "job-1"));
        assert!(!carries_marker(rec.pid, "job-2"));
        // A wrong start time, another job's marker or another boot signal nothing.
        let wrong_start = Recorded {
            start: rec.start + 1_000_000,
            ..rec.clone()
        };
        let other_job = Recorded {
            job_id: "job-2".into(),
            ..rec.clone()
        };
        let other_boot = Recorded {
            boot_id: "00000000-0000-0000-0000-000000000000".into(),
            ..rec.clone()
        };
        let grace = std::time::Duration::from_secs(1);
        assert_eq!(
            terminate_recorded_group(&wrong_start, grace).await,
            Termination::NotFound
        );
        assert_eq!(
            terminate_recorded_group(&other_job, grace).await,
            Termination::NotFound
        );
        assert_eq!(
            terminate_recorded_group(&other_boot, grace).await,
            Termination::OtherBoot
        );
        assert!(group_alive(rec.pid));
        assert_eq!(
            terminate_recorded_group(&rec, std::time::Duration::from_secs(5)).await,
            Termination::Signalled
        );
        let status = child.wait().unwrap();
        assert!(!status.success());
    }

    /// The old fallback signalled any group member that started after the recorded leader.
    /// A group whose members do not carry the job's marker is left alone.
    #[tokio::test]
    async fn unmarked_groups_are_never_signalled() {
        let (mut child, rec) = spawn_group("exec sleep 30", None);
        assert_eq!(
            terminate_recorded_group(&rec, std::time::Duration::from_secs(1)).await,
            Termination::NotFound
        );
        assert!(group_alive(rec.pid));
        signal_group(rec.pid, Signal::SIGKILL).unwrap();
        let _ = child.wait();
    }

    /// After the leader exits, a member that still carries the marker identifies the group;
    /// one that dropped it does not.
    #[tokio::test]
    async fn members_are_identified_by_their_marker_when_the_leader_is_gone() {
        // The members once the leader is reaped and the background child runs `sleep`
        // (before its last exec it may still be `sh` or `env`, with the marker).
        let wait_member = |pgid: i32| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                let members = group_members(pgid, 0);
                let sleeping = members.iter().all(|m| {
                    std::fs::read_to_string(format!("/proc/{m}/comm"))
                        .is_ok_and(|c| c.trim() == "sleep")
                });
                if !members.is_empty() && sleeping {
                    return members;
                }
                assert!(std::time::Instant::now() < deadline, "no member left");
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        };
        // The leader backgrounds a child and exits.
        let (mut child, rec) = spawn_group("sleep 30 & exit 0", Some("job-1"));
        let _ = child.wait();
        let members = wait_member(rec.pid);
        assert!(members.iter().all(|m| carries_marker(*m, "job-1")));
        assert_eq!(
            terminate_recorded_group(&rec, std::time::Duration::from_secs(5)).await,
            Termination::Signalled
        );
        assert!(group_members(rec.pid, 0).is_empty());

        let (mut child, rec) = spawn_group(
            &format!("env -u {JOB_MARKER_ENV} sleep 30 & exit 0"),
            Some("job-1"),
        );
        let _ = child.wait();
        let members = wait_member(rec.pid);
        assert_eq!(
            terminate_recorded_group(&rec, std::time::Duration::from_secs(1)).await,
            Termination::NotFound
        );
        assert_eq!(
            group_members(rec.pid, 0),
            members,
            "an unmarked member was signalled"
        );
        signal_group(rec.pid, Signal::SIGKILL).unwrap();
    }
}
