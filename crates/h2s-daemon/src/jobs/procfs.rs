//! Process identity from `/proc` (Linux; harness2 is Linux-only) and process-group signals.
//!
//! A pid alone is not an identity: after a restart the number may belong to another
//! process. Jobs record the leader's pid **and** its start time (clock ticks since boot,
//! `/proc/<pid>/stat` field 22); recovery signals a group only when both still match, or
//! when members of that group started no earlier than the recorded leader.

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

/// Terminate a job's process group found after a restart: only when the recorded leader
/// is still the same process, or members started no earlier than it. SIGTERM, then
/// SIGKILL after `grace`. Returns `true` when something was signalled.
pub async fn terminate_recorded_group(pid: i32, start: u64, grace: std::time::Duration) -> bool {
    let leader_matches = is_same_process(pid, start);
    if !leader_matches && group_members(pid, start).is_empty() {
        return false;
    }
    if signal_group(pid, Signal::SIGTERM).is_err() {
        return false;
    }
    let deadline = tokio::time::Instant::now() + grace;
    while tokio::time::Instant::now() < deadline {
        if group_members(pid, start).is_empty() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let _ = signal_group(pid, Signal::SIGKILL);
    true
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

    #[tokio::test]
    async fn terminates_a_recorded_group() {
        let mut cmd = std::process::Command::new("sleep");
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        let mut child = cmd.arg("30").spawn().unwrap();
        let pid = i32::try_from(child.id()).unwrap();
        let start = start_time(pid).unwrap();
        assert!(group_alive(pid));
        // A wrong start time must not signal anything.
        assert!(
            !terminate_recorded_group(pid, start + 1_000_000, std::time::Duration::from_secs(1))
                .await
        );
        assert!(terminate_recorded_group(pid, start, std::time::Duration::from_secs(5)).await);
        let status = child.wait().unwrap();
        assert!(!status.success());
    }
}
