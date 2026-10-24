//! End to end in a real browser: headless Firefox (via geckodriver) drives the real web UI
//! served by the real server, after the real harness2 demo ran. Elements are found only by
//! the `data-testid` hooks of contract v1.1. Screenshots go to
//! `target/e2e-artifacts/` for human review. Run with `cargo xtask e2e`.
//!
//! Extra environment: `GECKODRIVER` (default: `geckodriver` on PATH) and optionally
//! `FIREFOX_BIN` (passed to geckodriver as the browser binary).

// Test-only crate: a panic with a clear message IS how a failed expectation is reported here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use common::{Workbench, free_port, succeeded};
use fantoccini::{Client, ClientBuilder, Locator};
use h2w_api::{DemoMode, DemoParams, JobSpec};

struct Geckodriver {
    child: Child,
    url: String,
    _profile: tempfile::TempDir,
}

impl Drop for Geckodriver {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start_geckodriver() -> Geckodriver {
    let bin = std::env::var("GECKODRIVER").unwrap_or_else(|_| "geckodriver".into());
    let port = free_port();
    let profile = tempfile::tempdir().expect("profile tempdir");
    let mut child = Command::new(&bin)
        .args(["--port", &port.to_string(), "--host", "127.0.0.1"])
        // A fresh temporary profile root: never touch the developer's browser profile.
        .env("TMPDIR", profile.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|e| {
            panic!("cannot start {bin} ({e}); install geckodriver or set GECKODRIVER")
        });
    let url = format!("http://127.0.0.1:{port}");
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Geckodriver {
                child,
                url,
                _profile: profile,
            };
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // Never leave a driver process behind on the failure path.
    let _ = child.kill();
    let _ = child.wait();
    panic!("geckodriver did not listen on {url}");
}

async fn browser(driver: &Geckodriver, width: u32, height: u32) -> Client {
    let mut opts = serde_json::Map::new();
    opts.insert(
        "args".into(),
        serde_json::json!([
            "-headless",
            format!("--width={width}"),
            format!("--height={height}")
        ]),
    );
    if let Ok(firefox) = std::env::var("FIREFOX_BIN") {
        opts.insert("binary".into(), serde_json::Value::String(firefox));
    }
    let mut caps = serde_json::Map::new();
    caps.insert("browserName".into(), "firefox".into());
    caps.insert("moz:firefoxOptions".into(), serde_json::Value::Object(opts));
    ClientBuilder::rustls()
        .expect("rustls connector")
        .capabilities(caps)
        .connect(&driver.url)
        .await
        .expect("WebDriver session")
}

fn artifacts_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/e2e-artifacts");
    std::fs::create_dir_all(&dir).expect("artifacts dir");
    dir
}

async fn shot(c: &Client, name: &str) {
    let png = c.screenshot().await.expect("screenshot");
    std::fs::write(artifacts_dir().join(format!("{name}.png")), png).expect("write screenshot");
}

fn tid(id: &str) -> String {
    format!("[data-testid=\"{id}\"]")
}

async fn wait_for(c: &Client, css: &str) -> fantoccini::elements::Element {
    c.wait()
        .at_most(Duration::from_secs(20))
        .for_element(Locator::Css(css))
        .await
        .unwrap_or_else(|e| panic!("element {css} did not appear: {e}"))
}

/// Wait until the element at `css` contains `needle`. Content fetched after its element
/// appears (a job log, a library, a diff) must be waited for, not read at once: a slow CI
/// runner once read a job log before its lines had arrived.
async fn wait_for_text(c: &Client, css: &str, needle: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        // The element can be replaced while it renders; a failed read just tries again.
        let text = wait_for(c, css).await.text().await.unwrap_or_default();
        if text.contains(needle) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{css} never contained {needle:?}; last text: {:?}",
            text.chars().take(300).collect::<String>()
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test]
#[ignore = "end-to-end: run with `cargo xtask e2e`"]
async fn login_browse_the_demo_and_diff_a_harness() {
    let workbench = Workbench::start();
    let api = workbench.admin();
    let job = api
        .create_job(&JobSpec::Demo(DemoParams {
            experiment: "demo".into(),
            k: 2,
            mode: DemoMode::Both,
        }))
        .await;
    let job = api.wait_job(&job.id, Duration::from_secs(120)).await;
    assert!(succeeded(&job), "demo job ended {:?}", job.status);

    let driver = start_geckodriver();
    let c = browser(&driver, 1366, 900).await;
    let base = &workbench.base;

    // Unauthenticated visits land on the login page.
    c.goto(&format!("{base}/experiments")).await.expect("goto");
    let token = wait_for(&c, &tid("login-token")).await;
    shot(&c, "01-login").await;

    // A wrong token shows an error and does not log in.
    token
        .send_keys("h2w_wrongid_notavalidsecretnotavalidsecret00")
        .await
        .expect("type");
    wait_for(&c, &tid("login-submit"))
        .await
        .click()
        .await
        .expect("click");
    wait_for(&c, &tid("login-error")).await;

    // The right token logs in (session cookie) and returns to the page that was asked for,
    // so a shared deep link survives the sign-in.
    let token = wait_for(&c, &tid("login-token")).await;
    token.clear().await.expect("clear");
    token.send_keys(&workbench.admin_token).await.expect("type");
    wait_for(&c, &tid("login-submit"))
        .await
        .click()
        .await
        .expect("click");
    wait_for(
        &c,
        &format!("{}[data-name=\"demo\"]", tid("experiment-row")),
    )
    .await;
    let here = c.current_url().await.expect("url");
    assert_eq!(
        here.path(),
        "/experiments",
        "sign-in did not return to the deep link"
    );

    // The dashboard.
    wait_for(&c, &tid("nav-dashboard"))
        .await
        .click()
        .await
        .expect("click");
    wait_for(&c, &tid("dashboard")).await;
    let who = wait_for(&c, &tid("current-user"))
        .await
        .text()
        .await
        .expect("text");
    assert!(who.contains("e2e-admin"), "current user shows {who:?}");
    wait_for(&c, &tid("harness2-status")).await;
    shot(&c, "02-dashboard").await;

    // Experiments -> demo -> its sweep -> a task.
    wait_for(&c, &tid("nav-experiments"))
        .await
        .click()
        .await
        .expect("nav");
    wait_for(&c, "[data-testid=\"experiment-row\"][data-name=\"demo\"]")
        .await
        .click()
        .await
        .expect("open experiment");
    wait_for(
        &c,
        "[data-testid=\"sweep-row\"][data-sweep=\"demo-oc-agg2\"]",
    )
    .await
    .click()
    .await
    .expect("open sweep");
    wait_for(&c, "[data-testid=\"task-row\"][data-task=\"ledger-1\"]").await;
    shot(&c, "03-sweep-tasks").await;
    wait_for(&c, &tid("tab-library"))
        .await
        .click()
        .await
        .expect("library tab");
    // The library view shows the demo lesson.
    wait_for_text(&c, &tid("library-view"), "credits and debits").await;
    shot(&c, "04-sweep-library").await;
    wait_for(&c, &tid("tab-tasks"))
        .await
        .click()
        .await
        .expect("tasks tab");
    wait_for(&c, "[data-testid=\"task-row\"][data-task=\"ledger-1\"]")
        .await
        .click()
        .await
        .expect("open task");

    // The task page shows both modes, and the diff shows the demo's one real edit.
    wait_for(&c, "[data-testid=\"mode-section\"][data-mode=\"parallel\"]").await;
    wait_for(
        &c,
        "[data-testid=\"mode-section\"][data-mode=\"sequential\"]",
    )
    .await;
    shot(&c, "05-task").await;
    let url = format!("{base}/experiments/demo/sweeps/demo-oc-agg2/tasks/ledger-1");
    c.goto(&url)
        .await
        .expect("direct link works (SPA fallback)");
    wait_for(&c, &tid("diff-view")).await;
    // Choose base -> agg1 if the selects exist with those options.
    if let Ok(from) = c
        .find(Locator::Css(&format!(
            "{} option[value=\"base\"]",
            tid("diff-from")
        )))
        .await
    {
        from.click().await.expect("pick base");
    }
    if let Ok(to) = c
        .find(Locator::Css(&format!(
            "{} option[value=\"agg1\"]",
            tid("diff-to")
        )))
        .await
    {
        to.click().await.expect("pick agg1");
    }
    // The diff names the edited script.
    wait_for_text(&c, &tid("diff-view"), "summarize.py").await;
    shot(&c, "06-task-diff").await;

    // Jobs list and the finished job page.
    c.goto(&format!("{base}/jobs/{}", job.id))
        .await
        .expect("job page");
    let status = wait_for(&c, &tid("job-status"))
        .await
        .text()
        .await
        .expect("text");
    assert_eq!(status.trim(), "succeeded");
    // The job log is shown once its lines have been fetched.
    wait_for_text(&c, &tid("job-log"), "Local demo").await;
    shot(&c, "07-job").await;

    // Phone width renders without horizontal scrolling.
    c.set_window_size(390, 844).await.expect("resize");
    c.goto(&format!("{base}/")).await.expect("goto");
    wait_for(&c, &tid("dashboard")).await;
    let overflow = c
        .execute(
            "return document.documentElement.scrollWidth > window.innerWidth + 1;",
            vec![],
        )
        .await
        .expect("execute");
    assert_eq!(
        overflow,
        serde_json::Value::Bool(false),
        "horizontal scroll at phone width"
    );
    shot(&c, "08-dashboard-phone").await;

    c.close().await.expect("close browser");
}
