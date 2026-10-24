//! Container isolation: the exact `docker`/`podman run` argv for a job.
//!
//! Every runtime-level flag is a fixed literal chosen here; request data only ever appears
//! after the image name (the program and its already-validated arguments) or as validated
//! mount sources from server configuration.

use crate::{CoreError, Result};

/// Where the experiment directory is mounted inside the runner container.
pub const EXPERIMENT_MOUNT: &str = "/h2s/experiment";
/// Where the harness2 checkout (with its venv and `dependencies/`) is mounted.
pub const HARNESS2_MOUNT: &str = "/h2s/harness2";
/// Where per-job generated files (OpenCode config, key file) are mounted read-only.
pub const JOBFILES_MOUNT: &str = "/h2s/job";
/// Label put on every container the server starts; checked before kill/logs/rm.
pub const JOB_LABEL: &str = "io.github.harness2-studio.job";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime {
    Docker,
    Podman,
}

impl Runtime {
    #[must_use]
    pub fn program(self) -> &'static str {
        match self {
            Runtime::Docker => "docker",
            Runtime::Podman => "podman",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    /// Absolute host path from server configuration (validated: absolute, no `:`, `,` or
    /// control characters, not a symlink — the last checked by the server at run time).
    pub host: String,
    /// One of the fixed mount points above.
    pub container: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContainerSpec {
    pub runtime: Runtime,
    /// Pinned image reference (`name:tag` or `name@sha256:...`), from configuration.
    pub image: String,
    /// Job id; becomes the container name `h2s-job-<id>` and the label value.
    pub job_id: String,
    pub mounts: Vec<Mount>,
    /// Variable NAMES passed through with `-e NAME` (values come from the runtime's env).
    pub env_names: Vec<String>,
    /// Host path of a private (0600) file of `NAME=VALUE` lines passed with `--env-file`: the
    /// job's environment, secret values included, which then never reach the runtime CLI's
    /// own environment or command line. The server writes it into the job's directory.
    pub env_file: Option<String>,
    /// Fixed settings passed as `-e NAME=VALUE` (the egress proxy URL). Never a secret: the
    /// value is on the runtime's command line, which local users can read.
    pub env_values: Vec<(String, String)>,
    /// `none`, or the name of a pre-created network (e.g. one behind an egress proxy).
    pub network: String,
    pub cpus: Option<f64>,
    pub memory_mb: Option<u64>,
    pub pids_limit: u32,
    /// Working directory inside the container.
    pub workdir: String,
    /// `uid:gid` to run as (default: the image's non-root user). The server passes `0:0` only
    /// for a rootless runtime, where container root IS the unprivileged server user and the
    /// only id that can write the bind-mounted experiment (measured on rootless Docker).
    pub user: Option<String>,
}

/// Container name for a job.
fn container_name(job_id: &str) -> String {
    format!("h2s-job-{job_id}")
}

/// Job ids: `[A-Za-z0-9][A-Za-z0-9_.-]{0,127}` (a UUIDv7 fits).
fn job_id_ok(id: &str) -> bool {
    let b = id.as_bytes();
    match b.split_first() {
        Some((f, rest)) => {
            f.is_ascii_alphanumeric()
                && b.len() <= 128
                && rest
                    .iter()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
        }
        None => false,
    }
}

/// One `/`-separated repository path component: lowercase alphanumerics joined by `.`,
/// `_` or `-` runs (docker's reference grammar, slightly simplified).
fn repo_component_ok(c: &str) -> bool {
    let b = c.as_bytes();
    !b.is_empty()
        && b.iter().all(|x| {
            x.is_ascii_lowercase() || x.is_ascii_digit() || matches!(x, b'.' | b'_' | b'-')
        })
        && b.first().is_some_and(u8::is_ascii_alphanumeric)
        && b.last().is_some_and(u8::is_ascii_alphanumeric)
        && !c.contains("..")
}

fn registry_ok(c: &str) -> bool {
    let (host, port) = match c.rsplit_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (c, None),
    };
    let host_ok = !host.is_empty()
        && host.split('.').all(|l| {
            !l.is_empty()
                && l.bytes().all(|x| x.is_ascii_alphanumeric() || x == b'-')
                && !l.starts_with('-')
                && !l.ends_with('-')
        });
    let port_ok =
        port.is_none_or(|p| !p.is_empty() && p.len() <= 5 && p.bytes().all(|x| x.is_ascii_digit()));
    host_ok && port_ok
}

/// A pinned image reference: `[registry/]path[:tag][@sha256:<64 hex>]` with a tag or a
/// digest. The mutable tag `latest` is refused.
fn image_ok(image: &str) -> std::result::Result<(), &'static str> {
    if image.len() > 512 || image.starts_with('-') {
        return Err("image reference is malformed");
    }
    let (name_tag, digest) = match image.split_once('@') {
        Some((n, d)) => (n, Some(d)),
        None => (image, None),
    };
    if let Some(d) = digest {
        let hex = d
            .strip_prefix("sha256:")
            .ok_or("digest must be sha256:<64 hex>")?;
        if hex.len() != 64
            || !hex
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err("digest must be sha256:<64 hex>");
        }
    }
    let last_slash = name_tag.rfind('/').map_or(0, |i| i + 1);
    let (name, tag) = match name_tag.get(last_slash..).and_then(|t| t.rfind(':')) {
        Some(i) => {
            let split = last_slash + i;
            (
                name_tag.get(..split).unwrap_or(""),
                name_tag.get(split + 1..),
            )
        }
        None => (name_tag, None),
    };
    if let Some(t) = tag {
        let tb = t.as_bytes();
        let ok = tb.len() <= 128
            && tb
                .first()
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
            && tb
                .iter()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'));
        if !ok {
            return Err("image tag is malformed");
        }
        if t == "latest" {
            return Err("pin the image to a version tag or digest, not latest");
        }
    }
    if tag.is_none() && digest.is_none() {
        return Err("pin the image with a tag or a sha256 digest");
    }
    let mut comps: Vec<&str> = name.split('/').collect();
    if comps.len() > 1
        && let Some(first) = comps.first().copied()
        && (first.contains('.') || first.contains(':') || first == "localhost")
    {
        if !registry_ok(first) {
            return Err("image registry is malformed");
        }
        comps.remove(0);
    }
    if comps.is_empty() || !comps.iter().all(|c| repo_component_ok(c)) || name.len() > 255 {
        return Err("image name is malformed");
    }
    Ok(())
}

/// An absolute path without `:`, `,`, control characters or `..` components (not `/`).
fn host_path_ok(p: &str) -> bool {
    p.starts_with('/')
        && p.len() > 1
        && p.len() <= 4096
        && !p.contains([':', ','])
        && !p.chars().any(char::is_control)
        && !p.split('/').any(|c| c == "..")
}

fn env_var_name_ok(n: &str) -> bool {
    let b = n.as_bytes();
    match b.split_first() {
        Some((f, rest)) => {
            (f.is_ascii_alphabetic() || *f == b'_')
                && b.len() <= 128
                && rest.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
        }
        None => false,
    }
}

fn user_ok(u: &str) -> std::result::Result<(), &'static str> {
    let (uid, gid) = match u.split_once(':') {
        Some((a, b)) => (a, Some(b)),
        None => (u, None),
    };
    let num = |s: &str| !s.is_empty() && s.len() <= 10 && s.bytes().all(|c| c.is_ascii_digit());
    if !num(uid) || gid.is_some_and(|g| !num(g)) {
        return Err("user must be a numeric uid or uid:gid");
    }
    Ok(())
}

/// `<runtime> run --rm --name h2s-job-<id> --label <JOB_LABEL>=<id> --cap-drop ALL
/// --security-opt no-new-privileges --pids-limit N [--cpus] [--memory] --network NET
/// [-v ...] [--env-file F] [-e NAME ...] [-e NAME=VALUE ...] -w WORKDIR [--user U] <image>
/// <program> <args...>`.
///
/// The returned argv starts with the runtime program. Refused: an unpinned or malformed
/// image (including the `latest` tag), a job id outside `[A-Za-z0-9][A-Za-z0-9_.-]*`, a mount
/// point other than the three fixed ones or used twice, a writable job-files mount, a host
/// path (mount source or env file) that is not absolute or contains `:`, `,`, `..` or control
/// characters, a fixed
/// value with control characters (or one that repeats a passed-through name), the `host`
/// network, a non-numeric user, non-positive limits, and NUL bytes anywhere. A numeric uid 0 is
/// accepted for rootless runtimes (see [`ContainerSpec::user`]); `--cap-drop ALL` and
/// `no-new-privileges` apply regardless.
pub fn run_argv(spec: &ContainerSpec, program: &str, args: &[String]) -> Result<Vec<String>> {
    let mut errs: Vec<(String, String)> = Vec::new();
    let mut err = |field: &str, problem: &str| errs.push((field.to_owned(), problem.to_owned()));
    if let Err(p) = image_ok(&spec.image) {
        err("image", p);
    }
    if !job_id_ok(&spec.job_id) {
        err("job_id", "must be 1-128 letters, digits, '_', '.' or '-'");
    }
    let mut used: Vec<&str> = Vec::new();
    for m in &spec.mounts {
        if !host_path_ok(&m.host) {
            err(
                "mounts",
                "host paths must be absolute without ':', ',', '..' or control characters",
            );
        }
        let point = m.container.as_str();
        if ![EXPERIMENT_MOUNT, HARNESS2_MOUNT, JOBFILES_MOUNT].contains(&point) {
            err("mounts", "container mount points are fixed by the server");
        } else if used.contains(&point) {
            err("mounts", "a container mount point is used twice");
        }
        used.push(point);
        if point == JOBFILES_MOUNT && !m.read_only {
            err("mounts", "the job-files mount must be read-only");
        }
    }
    if spec.env_file.as_deref().is_some_and(|f| !host_path_ok(f)) {
        err(
            "env_file",
            "must be an absolute host path without ':', ',', '..' or control characters",
        );
    }
    if !spec.env_names.iter().all(|n| env_var_name_ok(n)) {
        err(
            "env_names",
            "variable names must match [A-Za-z_][A-Za-z0-9_]*",
        );
    }
    let values_ok = spec.env_values.iter().all(|(n, v)| {
        env_var_name_ok(n)
            && !spec.env_names.contains(n)
            && v.len() <= 4096
            && !v.chars().any(char::is_control)
    });
    if !values_ok {
        err(
            "env_values",
            "names must match [A-Za-z_][A-Za-z0-9_]* and not repeat env_names; values must \
             have no control characters",
        );
    }
    let net = spec.network.as_str();
    let net_ok = net == "none" || (job_id_ok(net) && net != "host");
    if !net_ok {
        err(
            "network",
            "use none or the name of a pre-created network (never host)",
        );
    }
    if let Some(c) = spec.cpus
        && !(c.is_finite() && c > 0.0 && c <= 1024.0)
    {
        err("cpus", "must be greater than 0 and at most 1024");
    }
    if spec.memory_mb.is_some_and(|m| m < 6) {
        err("memory_mb", "must be at least 6");
    }
    if spec.pids_limit == 0 || spec.pids_limit > 4_194_304 {
        err("pids_limit", "must be between 1 and 4194304");
    }
    if !(spec.workdir.starts_with('/')
        && spec.workdir.len() <= 4096
        && !spec.workdir.chars().any(char::is_control)
        && !spec.workdir.split('/').any(|c| c == ".."))
    {
        err(
            "workdir",
            "must be an absolute path without '..' or control characters",
        );
    }
    if let Some(u) = &spec.user
        && let Err(p) = user_ok(u)
    {
        err("user", p);
    }
    if program.is_empty()
        || program.starts_with('-')
        || program.len() > 4096
        || program.chars().any(char::is_control)
    {
        err(
            "program",
            "must be a program path that does not start with '-'",
        );
    }
    if args.iter().any(|a| a.contains('\0')) {
        err("args", "arguments must not contain NUL");
    }
    if !errs.is_empty() {
        return Err(if errs.len() == 1 {
            let (field, problem) = errs.remove(0);
            CoreError::Invalid { field, problem }
        } else {
            CoreError::InvalidMany(errs)
        });
    }

    let mut argv: Vec<String> = [
        spec.runtime.program(),
        "run",
        "--rm",
        "--name",
        &container_name(&spec.job_id),
        "--label",
        &format!("{JOB_LABEL}={}", spec.job_id),
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--pids-limit",
        &spec.pids_limit.to_string(),
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    if let Some(c) = spec.cpus {
        argv.push("--cpus".into());
        argv.push(format!("{c}"));
    }
    if let Some(m) = spec.memory_mb {
        argv.push("--memory".into());
        argv.push(format!("{m}m"));
    }
    argv.push("--network".into());
    argv.push(spec.network.clone());
    for m in &spec.mounts {
        argv.push("-v".into());
        argv.push(format!(
            "{}:{}{}",
            m.host,
            m.container,
            if m.read_only { ":ro" } else { "" }
        ));
    }
    if let Some(f) = &spec.env_file {
        argv.push("--env-file".into());
        argv.push(f.clone());
    }
    for n in &spec.env_names {
        argv.push("-e".into());
        argv.push(n.clone());
    }
    for (n, v) in &spec.env_values {
        argv.push("-e".into());
        argv.push(format!("{n}={v}"));
    }
    argv.push("-w".into());
    argv.push(spec.workdir.clone());
    if let Some(u) = &spec.user {
        argv.push("--user".into());
        argv.push(u.clone());
    }
    argv.push(spec.image.clone());
    argv.push(program.to_owned());
    argv.extend(args.iter().cloned());
    Ok(argv)
}

/// argv that prints the job label of container `h2s-job-<id>` (to verify before killing).
///
/// Empty when `job_id` is not a valid job id (nothing can be verified then).
#[must_use]
pub fn inspect_label_argv(runtime: Runtime, job_id: &str) -> Vec<String> {
    if !job_id_ok(job_id) {
        return Vec::new();
    }
    vec![
        runtime.program().to_owned(),
        "inspect".to_owned(),
        "--type".to_owned(),
        "container".to_owned(),
        "--format".to_owned(),
        format!("{{{{index .Config.Labels \"{JOB_LABEL}\"}}}}"),
        container_name(job_id),
    ]
}

/// argv that stops container `h2s-job-<id>` (call only after the label matched).
///
/// `stop --time 10`: SIGTERM, then SIGKILL after ten seconds, like host jobs. Empty when
/// `job_id` is not a valid job id.
#[must_use]
pub fn kill_argv(runtime: Runtime, job_id: &str) -> Vec<String> {
    if !job_id_ok(job_id) {
        return Vec::new();
    }
    vec![
        runtime.program().to_owned(),
        "stop".to_owned(),
        "--time".to_owned(),
        "10".to_owned(),
        container_name(job_id),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> ContainerSpec {
        ContainerSpec {
            runtime: Runtime::Docker,
            image: "ghcr.io/example/harness2-studio-runner:0.1.0".into(),
            job_id: "0192a3b4-c5d6-7e8f-9a0b-1c2d3e4f5a6b".into(),
            mounts: vec![
                Mount {
                    host: "/srv/h2s/experiments/e1".into(),
                    container: EXPERIMENT_MOUNT.into(),
                    read_only: false,
                },
                Mount {
                    host: "/srv/h2s/harness2".into(),
                    container: HARNESS2_MOUNT.into(),
                    read_only: true,
                },
                Mount {
                    host: "/srv/h2s/jobs/j1".into(),
                    container: JOBFILES_MOUNT.into(),
                    read_only: true,
                },
            ],
            env_names: vec!["HARNESS2_RUNS_DIR".into(), "OPENCODE_CONFIG".into()],
            env_file: Some("/srv/h2s/jobs/j1/container.env".into()),
            env_values: vec![("HTTPS_PROXY".into(), "http://egress:3128".into())],
            network: "h2s-egress".into(),
            cpus: Some(4.0),
            memory_mb: Some(8192),
            pids_limit: 2048,
            workdir: HARNESS2_MOUNT.into(),
            user: Some("1000:1000".into()),
        }
    }

    #[test]
    fn exact_argv() {
        let argv = run_argv(
            &spec(),
            "/h2s/harness2/.venv/bin/harness2",
            &["--bench".into(), "lab".into()],
        )
        .unwrap();
        let expected = [
            "docker",
            "run",
            "--rm",
            "--name",
            "h2s-job-0192a3b4-c5d6-7e8f-9a0b-1c2d3e4f5a6b",
            "--label",
            "io.github.harness2-studio.job=0192a3b4-c5d6-7e8f-9a0b-1c2d3e4f5a6b",
            "--cap-drop",
            "ALL",
            "--security-opt",
            "no-new-privileges",
            "--pids-limit",
            "2048",
            "--cpus",
            "4",
            "--memory",
            "8192m",
            "--network",
            "h2s-egress",
            "-v",
            "/srv/h2s/experiments/e1:/h2s/experiment",
            "-v",
            "/srv/h2s/harness2:/h2s/harness2:ro",
            "-v",
            "/srv/h2s/jobs/j1:/h2s/job:ro",
            "--env-file",
            "/srv/h2s/jobs/j1/container.env",
            "-e",
            "HARNESS2_RUNS_DIR",
            "-e",
            "OPENCODE_CONFIG",
            "-e",
            "HTTPS_PROXY=http://egress:3128",
            "-w",
            "/h2s/harness2",
            "--user",
            "1000:1000",
            "ghcr.io/example/harness2-studio-runner:0.1.0",
            "/h2s/harness2/.venv/bin/harness2",
            "--bench",
            "lab",
        ];
        assert_eq!(argv, expected);
    }

    /// Rootless runtimes map container root to the unprivileged host user, so `0:0` must be
    /// accepted (the hardening flags stay).
    #[test]
    fn rootless_root_user_is_accepted() {
        let mut s = spec();
        s.user = Some("0:0".into());
        let argv = run_argv(&s, "harness2", &[]).unwrap();
        let joined = argv.join(" ");
        assert!(joined.contains("--user 0:0"), "{joined}");
        assert!(
            joined.contains("--cap-drop ALL") && joined.contains("no-new-privileges"),
            "{joined}"
        );
    }

    #[test]
    fn refusals() {
        type Mutation = Box<dyn Fn(&mut ContainerSpec)>;
        let cases: Vec<(&str, Mutation)> = vec![
            ("image", Box::new(|s| s.image = "runner".into())),
            ("image", Box::new(|s| s.image = "runner:latest".into())),
            ("image", Box::new(|s| s.image = "--privileged".into())),
            ("image", Box::new(|s| s.image = "Runner:1".into())),
            ("job_id", Box::new(|s| s.job_id = "x;rm".into())),
            ("network", Box::new(|s| s.network = "host".into())),
            ("network", Box::new(|s| s.network = "container:abc".into())),
            ("user", Box::new(|s| s.user = Some("root".into()))),
            ("pids_limit", Box::new(|s| s.pids_limit = 0)),
            ("cpus", Box::new(|s| s.cpus = Some(f64::NAN))),
            ("memory_mb", Box::new(|s| s.memory_mb = Some(1))),
            ("workdir", Box::new(|s| s.workdir = "rel".into())),
            ("env_names", Box::new(|s| s.env_names = vec!["A=B".into()])),
            (
                "env_values",
                Box::new(|s| s.env_values = vec![("A=B".into(), "x".into())]),
            ),
            (
                "env_values",
                Box::new(|s| s.env_values = vec![("P".into(), "http://a\n-v /:/x".into())]),
            ),
            (
                "env_values",
                Box::new(|s| s.env_values = vec![("OPENCODE_CONFIG".into(), "/x".into())]),
            ),
            (
                "env_file",
                Box::new(|s| s.env_file = Some("relative/container.env".into())),
            ),
            (
                "env_file",
                Box::new(|s| s.env_file = Some("/srv/../etc/passwd".into())),
            ),
            (
                "env_file",
                Box::new(|s| s.env_file = Some("/srv/a\nb".into())),
            ),
            ("mounts", Box::new(|s| s.mounts[0].host = "/a:/b".into())),
            ("mounts", Box::new(|s| s.mounts[0].host = "/a,b".into())),
            (
                "mounts",
                Box::new(|s| s.mounts[0].host = "/a/../etc".into()),
            ),
            ("mounts", Box::new(|s| s.mounts[0].host = "/".into())),
            (
                "mounts",
                Box::new(|s| s.mounts[0].container = "/etc".into()),
            ),
            (
                "mounts",
                Box::new(|s| s.mounts[1].container = EXPERIMENT_MOUNT.into()),
            ),
            ("mounts", Box::new(|s| s.mounts[2].read_only = false)),
        ];
        for (field, mutate) in cases {
            let mut s = spec();
            mutate(&mut s);
            match run_argv(&s, "harness2", &[]) {
                Err(CoreError::Invalid { field: f, .. }) => assert_eq!(f, field),
                other => panic!("{field}: {other:?}"),
            }
        }
        assert!(run_argv(&spec(), "-x", &[]).is_err());
        assert!(run_argv(&spec(), "harness2", &["a\0b".into()]).is_err());
        for ok in [
            "harness2-studio-runner:0.1.0",
            "localhost:5000/runner:1",
            "registry.example.com/org/runner@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ] {
            let mut s = spec();
            s.image = ok.into();
            assert!(run_argv(&s, "harness2", &[]).is_ok(), "{ok}");
        }
    }

    #[test]
    fn label_and_kill() {
        assert_eq!(
            inspect_label_argv(Runtime::Podman, "j1"),
            vec![
                "podman",
                "inspect",
                "--type",
                "container",
                "--format",
                "{{index .Config.Labels \"io.github.harness2-studio.job\"}}",
                "h2s-job-j1"
            ]
        );
        assert_eq!(
            kill_argv(Runtime::Docker, "j1"),
            vec!["docker", "stop", "--time", "10", "h2s-job-j1"]
        );
        assert!(kill_argv(Runtime::Docker, "-rf").is_empty());
        assert!(inspect_label_argv(Runtime::Docker, "").is_empty());
    }
}
