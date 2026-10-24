//! The `h2studio` binary: output discipline for scripts (token as the only stdout line,
//! the `listening` line), init semantics, token management, config check, serve and the
//! egress proxy as processes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_h2studio");

fn cmd(home: &Path) -> Command {
    let mut c = Command::new(BIN);
    c.env("HOME", home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .env_remove("H2S_CONFIG")
        .env_remove("H2S_DATA_DIR")
        .env_remove("H2S_LISTEN")
        .env_remove("H2S_PUBLIC_URL")
        .env_remove("H2S_LOG_FORMAT")
        .env_remove("H2S_OTLP_ENDPOINT")
        .env("NO_COLOR", "1");
    c
}

fn run(home: &Path, args: &[&str]) -> Output {
    cmd(home).args(args).output().unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn is_token(s: &str) -> bool {
    h2s_daemon::auth::parse(s).is_some()
}

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    config: PathBuf,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    Env {
        config: root.join("cfg/h2studio.toml"),
        root,
        home,
        _dir: dir,
    }
}

impl Env {
    fn run(&self, args: &[&str]) -> Output {
        let mut all: Vec<&str> = args.to_vec();
        let c = self.config.to_str().unwrap();
        all.extend(["--config", c]);
        run(&self.home, &all)
    }

    fn init(&self) -> String {
        let data = self.root.join("data");
        let o = self.run(&["init", "--data-dir", data.to_str().unwrap()]);
        assert!(o.status.success(), "{}", stderr(&o));
        let out = stdout(&o);
        assert_eq!(
            out.lines().count(),
            1,
            "stdout must be exactly the token: {out:?}"
        );
        out.trim().to_owned()
    }
}

#[test]
fn version_and_openapi_print_to_stdout() {
    let e = env();
    let o = run(&e.home, &["version"]);
    assert!(o.status.success());
    assert!(stdout(&o).starts_with(&format!("h2studio {}", env!("CARGO_PKG_VERSION"))));
    let o = run(&e.home, &["openapi"]);
    assert!(o.status.success());
    let doc: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(doc["openapi"], "3.1.0");
    assert!(doc["paths"]["/api/v1/jobs"].is_object());
}

#[test]
fn init_prints_only_the_token_and_refuses_a_second_bootstrap() {
    let e = env();
    let token = e.init();
    assert!(is_token(&token), "{token}");
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&e.config), 0o600);
    assert_eq!(mode(e.config.parent().unwrap()), 0o700);
    let saved = e.config.parent().unwrap().join("admin.token");
    assert_eq!(mode(&saved), 0o600);
    assert_eq!(std::fs::read_to_string(&saved).unwrap().trim(), token);
    let cfg = std::fs::read_to_string(&e.config).unwrap();
    assert!(cfg.contains(&e.root.join("data").display().to_string()));
    assert!(e.root.join("data/studio.db").exists());

    // A second init refuses, prints nothing on stdout, and leaves the config alone.
    let o = e.run(&["init"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(o.stdout.is_empty());
    assert!(stderr(&o).contains("already initialised"), "{}", stderr(&o));
    assert_eq!(std::fs::read_to_string(&e.config).unwrap(), cfg);
    // --force mints another bootstrap token (still the only stdout line).
    let o = e.run(&["init", "--force"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let second = stdout(&o).trim().to_owned();
    assert!(is_token(&second) && second != token);
    assert_eq!(std::fs::read_to_string(&e.config).unwrap(), cfg);

    // token create / list / revoke.
    let o = e.run(&[
        "token",
        "create",
        "--name",
        "ci",
        "--role",
        "operator",
        "--expires-in-days",
        "7",
    ]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(stdout(&o).lines().count(), 1);
    let ci = stdout(&o).trim().to_owned();
    assert!(is_token(&ci));
    let o = e.run(&["token", "list", "--json"]);
    let list: Vec<h2s_api::TokenInfo> = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(list.len(), 3);
    assert!(
        list.iter()
            .any(|t| t.name == "ci" && t.expires_at.is_some())
    );
    let text = String::from_utf8(e.run(&["token", "list"]).stdout).unwrap();
    assert!(
        !text.contains(&ci[13..]) && !text.contains(&token[13..]),
        "a secret was listed"
    );
    let id = &ci[4..12];
    let o = e.run(&["token", "revoke", id]);
    assert!(o.status.success(), "{}", stderr(&o));
    let o = e.run(&["token", "revoke", id]);
    assert_eq!(o.status.code(), Some(1));
    let o = e.run(&["token", "create", "--name", "x", "--role", "root"]);
    assert!(!o.status.success());
    let o = e.run(&["token", "create", "--name", "bad\tname", "--role", "viewer"]);
    assert!(!o.status.success());
    // Every CLI token action was audited.
    let o = e.run(&["audit", "verify"]);
    assert!(o.status.success());
    assert!(stdout(&o).contains("audit chain OK"));
}

/// A read-only configuration directory (a config file bind-mounted into a container) must
/// not lose the token: it is already in the database, so a failed save cannot be retried.
#[test]
fn init_prints_the_token_even_when_it_cannot_save_a_copy() {
    let e = env();
    let dir = e.config.parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        &e.config,
        format!(
            "[paths]\ndata_dir = \"{}\"\n",
            e.root.join("data").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let o = e.run(&["init"]);
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(o.status.success(), "{}", stderr(&o));
    let token = stdout(&o).trim().to_owned();
    assert!(is_token(&token), "stdout must be the token: {token:?}");
    if !dir.join("admin.token").exists() {
        // Root ignores the mode bits; everyone else gets the warning.
        assert!(stderr(&o).contains("could not be saved"), "{}", stderr(&o));
    }
    let o = e.run(&["token", "list", "--json"]);
    let list: Vec<h2s_api::TokenInfo> = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(list.len(), 1);
}

#[test]
fn init_takes_the_data_dir_from_the_environment() {
    let e = env();
    let data = e.root.join("from-env");
    let o = cmd(&e.home)
        .args(["init", "--config", e.config.to_str().unwrap()])
        .env("H2S_DATA_DIR", &data)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(data.join("studio.db").exists());
}

#[test]
fn config_check_reports_problems() {
    let e = env();
    std::fs::create_dir_all(e.config.parent().unwrap()).unwrap();
    std::fs::write(
        &e.config,
        "[server]\nlisten = \"nope\"\n[paths]\ndata_dir = \"/srv/x\"\n",
    )
    .unwrap();
    let o = e.run(&["config", "check"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("server.listen"), "{}", stderr(&o));
    std::fs::write(
        &e.config,
        format!("[paths]\ndata_dir = \"{}\"\n", e.root.join("d").display()),
    )
    .unwrap();
    let o = e.run(&["config", "check"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).starts_with("configuration OK"));
    // A missing explicit config file is an error, not silent defaults.
    let o = run(
        &e.home,
        &["config", "check", "--config", "/nonexistent/h2studio.toml"],
    );
    assert_eq!(o.status.code(), Some(1));
}

/// Spawn a long-running subcommand and return it with the first stdout line.
fn spawn_until_line(mut c: Command) -> (std::process::Child, String) {
    let mut child = c
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(stdout).read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = rx
        .recv_timeout(Duration::from_secs(60))
        .expect("no output line");
    (child, line.trim().to_owned())
}

fn stop(mut child: std::process::Child) -> std::process::ExitStatus {
    let pid = nix::unistd::Pid::from_raw(i32::try_from(child.id()).unwrap());
    nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM).unwrap();
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        if let Some(s) = child.try_wait().unwrap() {
            return s;
        }
        assert!(
            Instant::now() < deadline,
            "the process did not stop on SIGTERM"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_prints_the_listening_line_and_stops_on_sigterm() {
    let e = env();
    let token = e.init();
    let mut c = cmd(&e.home);
    c.args(["serve", "--config", e.config.to_str().unwrap()])
        .env("H2S_LISTEN", "127.0.0.1:0");
    let (child, line) = spawn_until_line(c);
    let url = line
        .strip_prefix("h2studio listening on ")
        .unwrap_or_else(|| panic!("unexpected first line {line:?}"))
        .to_owned();
    assert!(url.starts_with("http://127.0.0.1:"), "{url}");
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let h = client.get(format!("{url}/healthz")).send().await.unwrap();
    assert_eq!(h.status(), 200);
    let me = client
        .get(format!("{url}/api/v1/auth/me"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), 200);
    let p: h2s_api::Principal = me.json().await.unwrap();
    assert_eq!(p.name, "initial admin");
    let status = tokio::task::spawn_blocking(move || stop(child))
        .await
        .unwrap();
    assert!(status.success(), "{status:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn egress_proxy_runs_as_a_subcommand() {
    let e = env();
    std::fs::create_dir_all(e.config.parent().unwrap()).unwrap();
    std::fs::write(
        &e.config,
        format!(
            "[paths]\ndata_dir = \"{}\"\n[egress]\nlisten = \"127.0.0.1:0\"\nallow = [\"api.example.com\"]\n",
            e.root.join("data").display()
        ),
    )
    .unwrap();
    let mut c = cmd(&e.home);
    c.args(["egress-proxy", "--config", e.config.to_str().unwrap()]);
    let (child, line) = spawn_until_line(c);
    // Non-dumpable: other processes of this user cannot read its environment.
    assert!(std::fs::read(format!("/proc/{}/environ", child.id())).is_err());
    let addr: std::net::SocketAddr = line
        .strip_prefix("h2studio egress proxy listening on ")
        .unwrap_or_else(|| panic!("unexpected first line {line:?}"))
        .parse()
        .unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(b"CONNECT not-allowed.example:443 HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&buf).starts_with("HTTP/1.1 403"));
    let status = tokio::task::spawn_blocking(move || stop(child))
        .await
        .unwrap();
    assert!(status.success(), "{status:?}");
}

/// What a job process sees of the server's environment, measured through a real server and a
/// stub `harness2-demo` that prints its environment: proxy, CA and package-tool variables
/// pass through (contract addendum v1.2); anything else does not.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jobs_inherit_only_the_allowed_server_variables() {
    let e = env();
    let h2 = e.root.join("harness2");
    let bin = h2.join(".venv/bin");
    std::fs::create_dir_all(&bin).unwrap();
    let stub = "#!/bin/sh\n# test stub: print the environment a job sees\nenv | sort\nexit 0\n";
    for name in ["harness2", "harness2-demo"] {
        std::fs::write(bin.join(name), stub).unwrap();
        std::fs::set_permissions(bin.join(name), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::create_dir_all(e.config.parent().unwrap()).unwrap();
    std::fs::write(
        &e.config,
        format!(
            "[paths]\ndata_dir = \"{}\"\nharness2_home = \"{}\"\nharness2_managed = false\n",
            e.root.join("data").display(),
            h2.display()
        ),
    )
    .unwrap();
    let token = e.init();
    let mut c = cmd(&e.home);
    c.args(["serve", "--config", e.config.to_str().unwrap()])
        .env("H2S_LISTEN", "127.0.0.1:0")
        .env("HTTPS_PROXY", "http://proxy.invalid:3128")
        .env("UV_PYTHON_INSTALL_DIR", "/opt/uv/python")
        .env("NPM_CONFIG_REGISTRY", "https://registry.example.com/")
        .env("H2S_TEST_CANARY", "server-only")
        .env("AWS_SECRET_ACCESS_KEY", "server-only");
    let (child, line) = spawn_until_line(c);
    let url = line
        .strip_prefix("h2studio listening on ")
        .unwrap_or_else(|| panic!("unexpected first line {line:?}"))
        .to_owned();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let job: h2s_api::Job = client
        .post(format!("{url}/api/v1/jobs"))
        .bearer_auth(&token)
        .header("x-h2s-request", "1")
        .json(&serde_json::json!({"kind": "demo", "params": {"experiment": "env", "k": 1}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let text = loop {
        let log: h2s_api::LogChunk = client
            .get(format!("{url}/api/v1/jobs/{}/log?limit=500", job.id))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if log.complete {
            break log
                .lines
                .iter()
                .map(|l| l.text.clone())
                .collect::<Vec<_>>()
                .join("\n");
        }
        assert!(Instant::now() < deadline, "the job did not finish");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let status = tokio::task::spawn_blocking(move || stop(child))
        .await
        .unwrap();
    assert!(status.success(), "{status:?}");
    for seen in [
        "HTTPS_PROXY=http://proxy.invalid:3128",
        "UV_PYTHON_INSTALL_DIR=/opt/uv/python",
        "NPM_CONFIG_REGISTRY=https://registry.example.com/",
        "HARNESS2_RUNS_DIR=",
    ] {
        assert!(text.contains(seen), "{seen} missing in:\n{text}");
    }
    assert!(
        !text.contains("server-only"),
        "a server-only variable leaked:\n{text}"
    );
}

/// A host-mode job runs as the server's user, but must not read the server's environment
/// (`H2S_SECRET_*`, the proxy secret): `serve` makes itself non-dumpable, which closes
/// `/proc/<pid>/environ` (and ptrace) to every other process of the same user.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jobs_cannot_read_the_server_environment() {
    let e = env();
    let h2 = e.root.join("harness2");
    let bin = h2.join(".venv/bin");
    std::fs::create_dir_all(&bin).unwrap();
    // The stub's parent is the server: it tries to read the server's environment.
    let stub = "#!/bin/sh\n# test stub: try to read the server's environment\n\
                if cat /proc/$PPID/environ >/dev/null 2>&1; then echo 'environ: readable'; \
                else echo 'environ: denied'; fi\n\
                if tr '\\0' '\\n' </proc/$PPID/environ 2>/dev/null | grep -q '^H2S_SECRET_CANARY='; \
                then echo 'canary: leaked'; fi\n\
                echo \"parent: $PPID\"\nexit 0\n";
    for name in ["harness2", "harness2-demo"] {
        std::fs::write(bin.join(name), stub).unwrap();
        std::fs::set_permissions(bin.join(name), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::create_dir_all(e.config.parent().unwrap()).unwrap();
    std::fs::write(
        &e.config,
        format!(
            "[paths]\ndata_dir = \"{}\"\nharness2_home = \"{}\"\nharness2_managed = false\n",
            e.root.join("data").display(),
            h2.display()
        ),
    )
    .unwrap();
    let token = e.init();
    let mut c = cmd(&e.home);
    c.args(["serve", "--config", e.config.to_str().unwrap()])
        .env("H2S_LISTEN", "127.0.0.1:0")
        .env("H2S_SECRET_CANARY", "canary-value-0123456789");
    let (child, line) = spawn_until_line(c);
    let server_pid = child.id();
    let url = line
        .strip_prefix("h2studio listening on ")
        .unwrap_or_else(|| panic!("unexpected first line {line:?}"))
        .to_owned();
    // The test runs as the same user (and is the server's parent): denied as well.
    let own = std::fs::read(format!("/proc/{server_pid}/environ"));
    assert!(
        own.is_err(),
        "the test process read the server's environment"
    );
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let job: h2s_api::Job = client
        .post(format!("{url}/api/v1/jobs"))
        .bearer_auth(&token)
        .header("x-h2s-request", "1")
        .json(&serde_json::json!({"kind": "demo", "params": {"experiment": "peek", "k": 1}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let text = loop {
        let log: h2s_api::LogChunk = client
            .get(format!("{url}/api/v1/jobs/{}/log?limit=500", job.id))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if log.complete {
            break log
                .lines
                .iter()
                .map(|l| l.text.clone())
                .collect::<Vec<_>>()
                .join("\n");
        }
        assert!(Instant::now() < deadline, "the job did not finish");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let status = tokio::task::spawn_blocking(move || stop(child))
        .await
        .unwrap();
    assert!(status.success(), "{status:?}");
    assert!(
        text.contains(&format!("parent: {server_pid}")),
        "the stub's parent is not the server:\n{text}"
    );
    assert!(text.contains("environ: denied"), "{text}");
    assert!(!text.contains("environ: readable"), "{text}");
    assert!(!text.contains("canary: leaked"), "{text}");
}
