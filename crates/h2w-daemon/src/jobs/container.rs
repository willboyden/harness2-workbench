//! Container runtime helpers: which user jobs run as, and label-verified cancellation.
//!
//! **User.** Jobs must run as the server's own host user so they can write the bind-mounted
//! experiment directory. With rootless Docker/Podman, container uid 0 maps to the server
//! user (and is unprivileged on the host) while other uids map to subordinate ids that
//! cannot write the mount; with a rootful daemon, the server's uid:gid is passed directly.
//! An explicit `[isolation.container] user` always wins. `--cap-drop ALL` and
//! `no-new-privileges` apply in every case (they are fixed flags of `h2w_core::isolation`).
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
        for name in h2w_core::env::BASE_ENV_PASSTHROUGH
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

/// The argv that prints whether a network is internal (`true` or `false`); the same Go
/// template works for Docker and Podman. `network` is validated by the configuration
/// (`config::network_ok`: it cannot start with `-`).
#[must_use]
pub fn network_internal_argv(runtime: RuntimeKind, network: &str) -> Vec<String> {
    vec![
        runtime.as_str().to_owned(),
        "network".to_owned(),
        "inspect".to_owned(),
        "--format".to_owned(),
        "{{.Internal}}".to_owned(),
        network.to_owned(),
    ]
}

/// Whether `network` is internal: without a route out, so container jobs reach the
/// internet only through the egress proxy.
///
/// # Errors
/// When the runtime cannot be asked or does not know the network.
pub async fn network_is_internal<R: CommandRunner>(
    runner: &R,
    runtime: RuntimeKind,
    network: &str,
) -> Result<bool, String> {
    let (code, out) = runner.run(network_internal_argv(runtime, network)).await?;
    if code != 0 {
        return Err(format!(
            "`{} network inspect {network}` failed (exit {code}); does the network exist?",
            runtime.as_str()
        ));
    }
    match out.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!(
            "`{} network inspect {network}` answered {:?}",
            runtime.as_str(),
            crate::util::truncate_chars(other, 80)
        )),
    }
}

/// The doctor check for `[isolation.container] proxy`: the jobs network must be internal,
/// or jobs can reach the internet around the proxy. `None` when no proxy is configured
/// (or isolation is not `container`).
pub async fn network_check<R: CommandRunner>(
    runner: &R,
    c: &crate::config::ContainerSection,
) -> Option<h2w_api::DoctorCheck> {
    if c.proxy.is_empty() || c.network == "none" {
        return None;
    }
    let (level, detail, remedy) = match network_is_internal(runner, c.runtime, &c.network).await {
        Ok(true) => (
            h2w_api::CheckLevel::Ok,
            format!(
                "network {} is internal: container jobs reach the internet only through the \
                 egress proxy {}",
                c.network, c.proxy
            ),
            None,
        ),
        Ok(false) => (
            h2w_api::CheckLevel::Warn,
            format!(
                "network {} is NOT internal: container jobs can reach the internet directly, \
                 around the egress proxy {}",
                c.network, c.proxy
            ),
            Some(format!(
                "Recreate it with `{} network create --internal {}` and attach the egress \
                 proxy to it and to a network with internet access.",
                c.runtime.as_str(),
                c.network
            )),
        ),
        Err(e) => (
            h2w_api::CheckLevel::Warn,
            format!(
                "could not tell whether network {} is internal: {e}",
                c.network
            ),
            Some(format!(
                "Create it with `{} network create --internal {}`.",
                c.runtime.as_str(),
                c.network
            )),
        ),
    };
    Some(h2w_api::DoctorCheck {
        id: "container_network".to_owned(),
        label: "Jobs network".to_owned(),
        level,
        detail,
        remedy,
        needed_for: vec!["container".to_owned()],
    })
}

/// The argv that prints the id of `image` when the runtime has it (exit 1 when it does
/// not). `image` is validated by the configuration (`config::image_is_pinned`).
#[must_use]
pub fn image_inspect_argv(runtime: RuntimeKind, image: &str) -> Vec<String> {
    vec![
        runtime.as_str().to_owned(),
        "image".to_owned(),
        "inspect".to_owned(),
        "--format".to_owned(),
        "{{.Id}}".to_owned(),
        image.to_owned(),
    ]
}

/// The doctor check `runner_image` for container isolation: the configured image must be
/// present, or every container job fails at start.
pub async fn runner_image_check<R: CommandRunner>(
    runner: &R,
    c: &crate::config::ContainerSection,
) -> h2w_api::DoctorCheck {
    let runtime = c.runtime.as_str();
    let build = format!(
        "Build it from a checkout of Harness² Workbench: `{runtime} build -f \
         deploy/docker/runner.Dockerfile -t {} .` (or pull it from your registry).",
        c.image
    );
    let (level, detail, remedy) = match runner.run(image_inspect_argv(c.runtime, &c.image)).await {
        Ok((0, _)) => (
            h2w_api::CheckLevel::Ok,
            format!("runner image {} is present", c.image),
            None,
        ),
        Ok((code, _)) => (
            h2w_api::CheckLevel::Fail,
            format!(
                "runner image {} is not present for {runtime} (`image inspect` exit {code}); \
                 container jobs cannot start",
                c.image
            ),
            Some(build),
        ),
        Err(e) => (
            h2w_api::CheckLevel::Warn,
            format!(
                "could not ask {runtime} about the runner image {}: {e}",
                c.image
            ),
            Some(build),
        ),
    };
    h2w_api::DoctorCheck {
        id: "runner_image".to_owned(),
        label: "Runner image".to_owned(),
        level,
        detail,
        remedy,
        needed_for: vec!["container".to_owned()],
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
    let inspect = h2w_core::isolation::inspect_label_argv(runtime.core(), job_id);
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
            "container h2w-job-{job_id} carries label {label:?}, not this job; refusing to stop it"
        ));
    }
    let kill = h2w_core::isolation::kill_argv(runtime.core(), job_id);
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
    async fn the_jobs_network_must_be_internal_when_a_proxy_is_set() {
        let mut c = crate::config::ContainerSection {
            network: "h2w-egress".into(),
            proxy: "http://egress:3128".into(),
            ..crate::config::ContainerSection::default()
        };
        let level = |check: Option<h2w_api::DoctorCheck>| check.map(|c| c.level);
        let f = Fake::new(vec![(0, "true\n")]);
        assert_eq!(
            level(network_check(&f, &c).await),
            Some(h2w_api::CheckLevel::Ok)
        );
        assert_eq!(
            f.calls.lock().unwrap()[0],
            vec![
                "docker",
                "network",
                "inspect",
                "--format",
                "{{.Internal}}",
                "h2w-egress"
            ]
        );
        let f = Fake::new(vec![(0, "false\n")]);
        let check = network_check(&f, &c).await.unwrap();
        assert_eq!(check.level, h2w_api::CheckLevel::Warn);
        assert!(check.detail.contains("NOT internal"), "{}", check.detail);
        assert!(check.remedy.unwrap().contains("--internal h2w-egress"));
        let f = Fake::new(vec![(1, "")]);
        let check = network_check(&f, &c).await.unwrap();
        assert_eq!(check.level, h2w_api::CheckLevel::Warn);
        assert!(
            check.detail.contains("does the network exist"),
            "{}",
            check.detail
        );
        // No proxy: nothing to check, nothing run.
        c.proxy.clear();
        let f = Fake::new(vec![]);
        assert!(network_check(&f, &c).await.is_none());
        assert!(f.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_runner_image_must_be_present_in_container_mode() {
        let c = crate::config::ContainerSection {
            image: "harness2-workbench-runner:0.1.0".into(),
            ..crate::config::ContainerSection::default()
        };
        let f = Fake::new(vec![(0, "sha256:0123\n")]);
        let check = runner_image_check(&f, &c).await;
        assert_eq!(
            (check.id.as_str(), check.level),
            ("runner_image", h2w_api::CheckLevel::Ok)
        );
        assert_eq!(
            f.calls.lock().unwrap()[0],
            vec![
                "docker",
                "image",
                "inspect",
                "--format",
                "{{.Id}}",
                "harness2-workbench-runner:0.1.0"
            ]
        );
        let f = Fake::new(vec![(1, "")]);
        let check = runner_image_check(&f, &c).await;
        assert_eq!(check.level, h2w_api::CheckLevel::Fail);
        assert!(
            check
                .remedy
                .unwrap()
                .contains("deploy/docker/runner.Dockerfile -t harness2-workbench-runner:0.1.0")
        );
        let f = Fake::new(vec![]);
        assert_eq!(
            runner_image_check(&f, &c).await.level,
            h2w_api::CheckLevel::Warn
        );
    }

    #[tokio::test]
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
