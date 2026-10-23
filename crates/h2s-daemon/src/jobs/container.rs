//! Container runtime helpers: which user jobs run as, and label-verified cancellation.
//!
//! **User.** Jobs must run as the server's own host user so they can write the bind-mounted
//! experiment directory. With rootless Docker/Podman, container uid 0 maps to the server
//! user (and is unprivileged on the host) while other uids map to subordinate ids that
//! cannot write the mount; with a rootful daemon, the server's uid:gid is passed directly.
//! An explicit `[isolation.container] user` always wins. `--cap-drop ALL` and
//! `no-new-privileges` apply in every case (they are fixed flags of `h2s_core::isolation`).
//!
//! **Cancellation.** Stopping the runtime CLI does not stop its container. Before any
//! stop, the container's job label is read and compared with the job id; only an exact
//! match is stopped.

use std::future::Future;
use std::time::Duration;

use crate::config::RuntimeKind;

/// Runs a short command (no shell) and returns `(exit code, stdout)`.
pub trait CommandRunner: Send + Sync {
    fn run(&self, argv: Vec<String>) -> impl Future<Output = Result<(i32, String), String>> + Send;
}

/// The real runner: `tokio::process`, cleared environment plus the runtime's variables,
/// 30 s timeout.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemRunner;

/// Variables a container runtime CLI needs to find its daemon or rootless state.
pub const RUNTIME_ENV_PASSTHROUGH: &[&str] = &[
    "DOCKER_HOST",
    "DOCKER_CONTEXT",
    "DOCKER_CONFIG",
    "XDG_RUNTIME_DIR",
    "CONTAINER_HOST",
    "CONTAINERS_CONF",
    "REGISTRY_AUTH_FILE",
];

impl CommandRunner for SystemRunner {
    async fn run(&self, argv: Vec<String>) -> Result<(i32, String), String> {
        let (program, args) = argv.split_first().ok_or("empty command")?;
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args)
            .env_clear()
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        for name in h2s_core::env::BASE_ENV_PASSTHROUGH
            .iter()
            .chain(RUNTIME_ENV_PASSTHROUGH)
        {
            if let Some(v) = std::env::var_os(name) {
                cmd.env(name, v);
            }
        }
        let out = tokio::time::timeout(Duration::from_secs(30), cmd.output())
            .await
            .map_err(|_| format!("{program} did not answer within 30 s"))?
            .map_err(|e| format!("could not run {program}: {e}"))?;
        let mut stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        stdout.truncate(64 * 1024);
        Ok((out.status.code().unwrap_or(-1), stdout))
    }
}

/// The argv that tells whether the runtime runs rootless.
#[must_use]
pub fn rootless_probe_argv(runtime: RuntimeKind) -> Vec<String> {
    match runtime {
        RuntimeKind::Docker => vec![
            "docker".into(),
            "info".into(),
            "--format".into(),
            "{{range .SecurityOptions}}{{.}} {{end}}".into(),
        ],
        RuntimeKind::Podman => vec![
            "podman".into(),
            "info".into(),
            "--format".into(),
            "{{.Host.Security.Rootless}}".into(),
        ],
    }
}

/// Interpret the probe's output.
#[must_use]
pub fn parse_rootless(runtime: RuntimeKind, stdout: &str) -> bool {
    match runtime {
        RuntimeKind::Docker => stdout.split_whitespace().any(|o| o == "name=rootless"),
        RuntimeKind::Podman => stdout.trim().eq_ignore_ascii_case("true"),
    }
}

/// The `--user` value for jobs: the configured one, else `0:0` (rootless) or the server's
/// `uid:gid` (rootful).
///
/// # Errors
/// When the runtime cannot be asked.
pub async fn resolve_user<R: CommandRunner>(
    runner: &R,
    runtime: RuntimeKind,
    configured: &str,
) -> Result<String, String> {
    if !configured.is_empty() {
        return Ok(configured.to_owned());
    }
    let (code, out) = runner.run(rootless_probe_argv(runtime)).await?;
    if code != 0 {
        return Err(format!(
            "`{} info` failed (exit {code}); is the container runtime running?",
            runtime.as_str()
        ));
    }
    if parse_rootless(runtime, &out) {
        Ok("0:0".to_owned())
    } else {
        Ok(format!(
            "{}:{}",
            nix::unistd::getuid().as_raw(),
            nix::unistd::getgid().as_raw()
        ))
    }
}

/// Stop the container of `job_id` only if its job label matches. Returns `Ok(true)` when it
/// was stopped, `Ok(false)` when no matching container exists.
///
/// # Errors
/// When the runtime cannot be asked, or the label belongs to something else.
pub async fn verify_and_stop<R: CommandRunner>(
    runner: &R,
    runtime: RuntimeKind,
    job_id: &str,
) -> Result<bool, String> {
    let inspect = h2s_core::isolation::inspect_label_argv(runtime.core(), job_id);
    if inspect.is_empty() {
        return Err("the container inspect command is not available in this build".into());
    }
    let (code, out) = runner.run(inspect).await?;
    if code != 0 {
        // No such container (it already exited and was removed by --rm).
        return Ok(false);
    }
    let label = out.trim();
    if label != job_id {
        return Err(format!(
            "container h2s-job-{job_id} carries label {label:?}, not this job; refusing to stop it"
        ));
    }
    let kill = h2s_core::isolation::kill_argv(runtime.core(), job_id);
    if kill.is_empty() {
        return Err("the container stop command is not available in this build".into());
    }
    let (code, _) = runner.run(kill).await?;
    Ok(code == 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Fake {
        replies: Mutex<Vec<(i32, String)>>,
        calls: Mutex<Vec<Vec<String>>>,
    }

    impl Fake {
        fn new(replies: Vec<(i32, &str)>) -> Fake {
            Fake {
                replies: Mutex::new(
                    replies
                        .into_iter()
                        .map(|(c, s)| (c, s.to_owned()))
                        .rev()
                        .collect(),
                ),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl CommandRunner for Fake {
        async fn run(&self, argv: Vec<String>) -> Result<(i32, String), String> {
            self.calls.lock().unwrap().push(argv);
            self.replies
                .lock()
                .unwrap()
                .pop()
                .ok_or_else(|| "no reply".to_owned())
        }
    }

    #[test]
    fn rootless_detection_parses_both_runtimes() {
        assert!(parse_rootless(
            RuntimeKind::Docker,
            "name=seccomp,profile=builtin name=rootless name=cgroupns "
        ));
        assert!(!parse_rootless(
            RuntimeKind::Docker,
            "name=apparmor name=seccomp,profile=builtin "
        ));
        assert!(parse_rootless(RuntimeKind::Podman, "true\n"));
        assert!(!parse_rootless(RuntimeKind::Podman, "false\n"));
    }

    #[tokio::test]
    async fn user_resolution() {
        let f = Fake::new(vec![]);
        assert_eq!(
            resolve_user(&f, RuntimeKind::Docker, "1234:5678")
                .await
                .unwrap(),
            "1234:5678"
        );
        assert!(f.calls.lock().unwrap().is_empty());

        let f = Fake::new(vec![(0, "name=seccomp name=rootless ")]);
        assert_eq!(
            resolve_user(&f, RuntimeKind::Docker, "").await.unwrap(),
            "0:0"
        );
        assert_eq!(
            f.calls.lock().unwrap()[0],
            rootless_probe_argv(RuntimeKind::Docker)
        );

        let f = Fake::new(vec![(0, "name=seccomp ")]);
        let me = format!(
            "{}:{}",
            nix::unistd::getuid().as_raw(),
            nix::unistd::getgid().as_raw()
        );
        assert_eq!(resolve_user(&f, RuntimeKind::Docker, "").await.unwrap(), me);

        let f = Fake::new(vec![(1, "")]);
        assert!(resolve_user(&f, RuntimeKind::Podman, "").await.is_err());
    }

    #[tokio::test]
    #[ignore = "needs feat/core"]
    async fn stops_only_matching_labels() {
        let f = Fake::new(vec![(0, "job-1\n"), (0, "")]);
        assert!(
            verify_and_stop(&f, RuntimeKind::Docker, "job-1")
                .await
                .unwrap()
        );
        assert_eq!(f.calls.lock().unwrap().len(), 2);

        let f = Fake::new(vec![(0, "someone-else\n")]);
        assert!(
            verify_and_stop(&f, RuntimeKind::Docker, "job-1")
                .await
                .is_err()
        );
        assert_eq!(f.calls.lock().unwrap().len(), 1, "no stop after a mismatch");

        let f = Fake::new(vec![(1, "")]);
        assert!(
            !verify_and_stop(&f, RuntimeKind::Docker, "job-1")
                .await
                .unwrap()
        );
    }
}
