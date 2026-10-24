//! Shared harness for the end-to-end suites: starts the REAL `h2workbench` binary against a REAL
//! harness2 checkout in a throwaway data directory, and talks to it over HTTP.
//!
//! Required environment (set by `cargo xtask e2e`):
//! - `H2W_E2E_BIN_DIR`: directory holding the built `h2workbench` and `h2workbench-mcp` binaries;
//! - `H2W_E2E_HARNESS2_HOME`: a harness2 checkout with its `.venv` (see the crate README).
//!
//! A missing prerequisite FAILS the test with an explanation; it never skips silently,
//! because "skipped" and "passed" look the same in a summary line.

// Test-only helpers: a panic with a clear message IS how a failed expectation is reported.
#![allow(
    dead_code,
    unreachable_pub,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use h2w_api::{Job, JobSpec, JobStatus};
use serde::de::DeserializeOwned;

pub const CSRF_HEADER: &str = h2w_api::CSRF_HEADER;

/// Paths to the binaries and the harness2 checkout, from the environment.
pub struct Env {
    pub bin_dir: PathBuf,
    pub harness2_home: PathBuf,
}

impl Env {
    pub fn from_env() -> Env {
        let bin_dir = PathBuf::from(std::env::var("H2W_E2E_BIN_DIR").unwrap_or_else(|_| {
            panic!("H2W_E2E_BIN_DIR is not set; run the suite with `cargo xtask e2e`")
        }));
        let harness2_home = PathBuf::from(std::env::var("H2W_E2E_HARNESS2_HOME").unwrap_or_else(|_| {
            panic!("H2W_E2E_HARNESS2_HOME is not set; point it at a harness2 checkout with .venv (crates/h2w-e2e/README.md)")
        }));
        for bin in ["h2workbench", "h2workbench-mcp"] {
            assert!(
                bin_dir.join(bin).is_file(),
                "{} not found in {}",
                bin,
                bin_dir.display()
            );
        }
        assert!(
            harness2_home.join(".venv/bin/harness2-demo").is_file(),
            "{} has no .venv/bin/harness2-demo; install harness2 there first",
            harness2_home.display()
        );
        Env {
            bin_dir,
            harness2_home,
        }
    }

    pub fn workbench(&self) -> PathBuf {
        self.bin_dir.join("h2workbench")
    }

    pub fn mcp(&self) -> PathBuf {
        self.bin_dir.join("h2workbench-mcp")
    }
}

/// A running server, stopped (SIGKILL + wait) when dropped.
pub struct Workbench {
    pub base: String,
    pub port: u16,
    pub admin_token: String,
    pub config: PathBuf,
    pub dir: tempfile::TempDir,
    pub env: Env,
    child: Child,
    /// Everything the server printed to stderr, for leak checks.
    stderr_lines: mpsc::Receiver<String>,
}

impl Drop for Workbench {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A free loopback port (bind :0, read it, release it; the race window is tiny in tests).
pub fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
    l.local_addr().expect("local addr").port()
}

impl Workbench {
    /// Write a config for a throwaway data dir, mint an admin token through the CLI, start
    /// `h2workbench serve` and wait for its "listening on" line.
    pub fn start() -> Workbench {
        let env = Env::from_env();
        let dir = tempfile::tempdir().expect("tempdir");
        let port = free_port();
        let data = dir.path().join("data");
        let experiments = dir.path().join("experiments");
        let config = dir.path().join("h2workbench.toml");
        let toml = format!(
            r#"[server]
listen = "127.0.0.1:{port}"
public_url = "http://127.0.0.1:{port}"

[paths]
data_dir = "{data}"
experiments_dir = "{experiments}"
harness2_home = "{home}"
harness2_managed = false

[isolation]
mode = "host"
preacknowledge_host_risks = true

[telemetry]
log_format = "json"
log_level = "info"
"#,
            data = data.display(),
            experiments = experiments.display(),
            home = env.harness2_home.display(),
        );
        std::fs::write(&config, toml).expect("write config");

        let admin_token = cli_token(&env, &config, "e2e-admin", "admin");

        let mut child = Command::new(env.workbench())
            .arg("serve")
            .arg("--config")
            .arg(&config)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn h2workbench serve");

        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");
        let (tx_out, rx_out) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let _ = tx_out.send(line);
            }
        });
        let (tx_err, rx_err) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let _ = tx_err.send(line);
            }
        });

        let deadline = Instant::now() + Duration::from_secs(30);
        let mut base = None;
        while Instant::now() < deadline {
            if let Ok(line) = rx_out.recv_timeout(Duration::from_millis(200))
                && let Some(url) = line.strip_prefix("h2workbench listening on ")
            {
                base = Some(url.trim().to_owned());
                break;
            }
            if let Ok(Some(status)) = child.try_wait() {
                let errs: Vec<String> = rx_err.try_iter().collect();
                panic!(
                    "h2workbench serve exited early with {status}:\n{}",
                    errs.join("\n")
                );
            }
        }
        let base = base
            .unwrap_or_else(|| panic!("h2workbench did not print its listening line within 30 s"));
        assert_eq!(
            base,
            format!("http://127.0.0.1:{port}"),
            "server bound an unexpected address"
        );

        Workbench {
            base,
            port,
            admin_token,
            config,
            dir,
            env,
            child,
            stderr_lines: rx_err,
        }
    }

    /// Mint another token with the given role.
    pub fn token(&self, name: &str, role: &str) -> String {
        cli_token(&self.env, &self.config, name, role)
    }

    pub fn api(&self, token: &str) -> Api {
        Api {
            base: self.base.clone(),
            token: token.to_owned(),
            http: http_client(),
        }
    }

    pub fn admin(&self) -> Api {
        self.api(&self.admin_token)
    }

    /// Server stderr collected so far (drains the channel).
    pub fn stderr_so_far(&self) -> Vec<String> {
        self.stderr_lines.try_iter().collect()
    }

    pub fn experiments_dir(&self) -> PathBuf {
        self.dir.path().join("experiments")
    }

    pub fn data_dir(&self) -> PathBuf {
        self.dir.path().join("data")
    }
}

fn cli_token(env: &Env, config: &Path, name: &str, role: &str) -> String {
    let out = Command::new(env.workbench())
        .args(["token", "create", "--config"])
        .arg(config)
        .args(["--name", name, "--role", role])
        .output()
        .expect("run h2workbench token create");
    assert!(
        out.status.success(),
        "h2workbench token create failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).expect("utf-8 token");
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        lines.len(),
        1,
        "token create must print exactly one stdout line, got {lines:?}"
    );
    let token = lines[0].trim().to_owned();
    assert!(
        token.starts_with(h2w_api::TOKEN_PREFIX),
        "unexpected token format"
    );
    token
}

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(60))
        .build()
        .expect("reqwest client")
}

/// A thin authenticated client for the REST API.
pub struct Api {
    pub base: String,
    pub token: String,
    pub http: reqwest::Client,
}

impl Api {
    pub fn url(&self, path: &str) -> String {
        format!("{}{}{}", self.base, h2w_api::API_PREFIX, path)
    }

    pub async fn get_raw(&self, path: &str) -> reqwest::Response {
        self.http
            .get(self.url(path))
            .bearer_auth(&self.token)
            .send()
            .await
            .expect("GET")
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> T {
        let resp = self.get_raw(path).await;
        let status = resp.status();
        let body = resp.text().await.expect("body");
        assert!(status.is_success(), "GET {path} -> {status}: {body}");
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("GET {path}: bad JSON ({e}): {body}"))
    }

    pub async fn post_raw<B: serde::Serialize>(&self, path: &str, body: &B) -> reqwest::Response {
        self.http
            .post(self.url(path))
            .bearer_auth(&self.token)
            .header(CSRF_HEADER, "1")
            .json(body)
            .send()
            .await
            .expect("POST")
    }

    pub async fn post<B: serde::Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> T {
        let resp = self.post_raw(path, body).await;
        let status = resp.status();
        let text = resp.text().await.expect("body");
        assert!(status.is_success(), "POST {path} -> {status}: {text}");
        serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("POST {path}: bad JSON ({e}): {text}"))
    }

    pub async fn put_raw<B: serde::Serialize>(&self, path: &str, body: &B) -> reqwest::Response {
        self.http
            .put(self.url(path))
            .bearer_auth(&self.token)
            .header(CSRF_HEADER, "1")
            .json(body)
            .send()
            .await
            .expect("PUT")
    }

    pub async fn create_job(&self, spec: &JobSpec) -> Job {
        self.post("/jobs", spec).await
    }

    /// Poll a job until it is terminal or the timeout passes.
    pub async fn wait_job(&self, id: &str, timeout: Duration) -> Job {
        let deadline = Instant::now() + timeout;
        loop {
            let job: Job = self.get(&format!("/jobs/{id}")).await;
            if job.status.is_terminal() {
                return job;
            }
            assert!(
                Instant::now() < deadline,
                "job {id} still {:?} after {timeout:?}",
                job.status
            );
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}

/// True when a job ended the way a harness2 command that fully succeeded does.
pub fn succeeded(job: &Job) -> bool {
    job.status == JobStatus::Succeeded
}
