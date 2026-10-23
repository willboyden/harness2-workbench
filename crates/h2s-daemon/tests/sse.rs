//! `GET /api/v1/jobs/{id}/events`: unnamed JSON frames, snapshot first, log ids, replay,
//! terminal close, connection cap.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::time::Duration;

use common::*;
use futures::StreamExt;
use h2s_api::{Job, JobEvent, JobStatus};

async fn read_stream(resp: reqwest::Response, secs: u64) -> String {
    let mut body = Vec::new();
    let mut stream = resp.bytes_stream();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        match tokio::time::timeout_at(deadline, stream.next()).await {
            Ok(Some(Ok(chunk))) => body.extend_from_slice(&chunk),
            Ok(Some(Err(e))) => panic!("stream error: {e}"),
            Ok(None) => break,
            Err(_) => panic!("the event stream did not end"),
        }
    }
    String::from_utf8(body).unwrap()
}

fn events(text: &str) -> Vec<(SseFrame, JobEvent)> {
    parse_sse(text)
        .into_iter()
        .map(|f| {
            let ev: JobEvent = serde_json::from_str(&f.data)
                .unwrap_or_else(|e| panic!("frame is not one JobEvent ({e}): {:?}", f.raw));
            (f, ev)
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stream_is_unnamed_json_frames_from_snapshot_to_terminal_status() {
    let t = TestServer::start_with(|s| s.sleep_secs = 0.5).await;
    let job: Job = t
        .create_job(&t.operator, demo_spec("slow-sse"))
        .await
        .json()
        .await
        .unwrap();
    let resp = t
        .get(&format!("/api/v1/jobs/{}/events", job.id), Some(&t.viewer))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(
        resp.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    assert_eq!(resp.headers()["cache-control"], "no-store");
    let text = read_stream(resp, 30).await;
    // Contract addendum v1.1: no named events, ever.
    assert!(
        !text.lines().any(|l| l.starts_with("event:")),
        "named event found in:\n{text}"
    );
    let evs = events(&text);
    assert!(matches!(evs[0].1, JobEvent::Snapshot { .. }), "{text}");
    let mut saw_log = false;
    let mut saw_step = false;
    let mut saw_progress = false;
    let mut last_n = None;
    for (f, e) in &evs {
        match e {
            JobEvent::Log { line } => {
                saw_log = true;
                assert_eq!(f.id.as_deref(), Some(line.n.to_string().as_str()));
                if let Some(prev) = last_n {
                    assert_eq!(line.n, prev + 1, "log lines out of order");
                }
                last_n = Some(line.n);
            }
            JobEvent::Step { .. } => saw_step = true,
            JobEvent::Progress { .. } => saw_progress = true,
            _ => assert!(f.id.is_none(), "only log events carry ids"),
        }
    }
    assert!(saw_log && saw_step && saw_progress, "{text}");
    match &evs.last().unwrap().1 {
        JobEvent::Status {
            status, exit_code, ..
        } => {
            assert_eq!(*status, JobStatus::Succeeded);
            assert_eq!(*exit_code, Some(0));
        }
        other => panic!("last frame {other:?}"),
    }
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn finished_jobs_get_a_snapshot_and_replays_follow_last_event_id() {
    let t = TestServer::start().await;
    let job: Job = t
        .create_job(&t.operator, demo_spec("chatty-r"))
        .await
        .json()
        .await
        .unwrap();
    let done = t.wait_terminal(&job.id, 60).await;
    let resp = t
        .get(&format!("/api/v1/jobs/{}/events", job.id), Some(&t.viewer))
        .send()
        .await
        .unwrap();
    let evs = events(&read_stream(resp, 10).await);
    assert_eq!(evs.len(), 1);
    match &evs[0].1 {
        JobEvent::Snapshot { job } => assert_eq!(job.status, JobStatus::Succeeded),
        other => panic!("{other:?}"),
    }

    let resp = t
        .get(&format!("/api/v1/jobs/{}/events", job.id), Some(&t.viewer))
        .header("last-event-id", "1000")
        .send()
        .await
        .unwrap();
    let evs = events(&read_stream(resp, 10).await);
    let logs: Vec<u64> = evs
        .iter()
        .filter_map(|(_, e)| match e {
            JobEvent::Log { line } => Some(line.n),
            _ => None,
        })
        .collect();
    assert_eq!(logs.first(), Some(&1001));
    assert_eq!(logs.last(), Some(&(done.log_lines - 1)));
    assert_eq!(logs.len() as u64, done.log_lines - 1001);
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn event_streams_are_capped() {
    let t = TestServer::start_with(|s| s.file.server.sse_max_connections = 1).await;
    let job: Job = t
        .create_job(&t.operator, demo_spec("sleep-cap"))
        .await
        .json()
        .await
        .unwrap();
    t.wait_job(&job.id, 30, |j| j.status == JobStatus::Running)
        .await;
    let first = t
        .get(&format!("/api/v1/jobs/{}/events", job.id), Some(&t.viewer))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), 200);
    let second = t
        .get(&format!("/api/v1/jobs/{}/events", job.id), Some(&t.viewer))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), 503);
    let m = t
        .get("/metrics", Some(&t.viewer))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(m.contains("h2s_sse_connections 1"), "{m}");
    drop(first);
    // The slot is released once the server notices the closed connection.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let again = t
            .get(&format!("/api/v1/jobs/{}/events", job.id), Some(&t.viewer))
            .send()
            .await
            .unwrap();
        if again.status() == 200 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the slot was never released"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    t.post(
        &format!("/api/v1/jobs/{}/cancel", job.id),
        Some(&t.operator),
    )
    .send()
    .await
    .unwrap();
    t.wait_terminal(&job.id, 30).await;
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_ends_the_stream_with_the_cancelled_status() {
    let t = TestServer::start().await;
    let job: Job = t
        .create_job(&t.operator, demo_spec("sleep-sc"))
        .await
        .json()
        .await
        .unwrap();
    t.wait_job(&job.id, 30, |j| j.status == JobStatus::Running)
        .await;
    let resp = t
        .get(&format!("/api/v1/jobs/{}/events", job.id), Some(&t.viewer))
        .send()
        .await
        .unwrap();
    let reader = tokio::spawn(read_stream(resp, 30));
    tokio::time::sleep(Duration::from_millis(300)).await;
    t.post(
        &format!("/api/v1/jobs/{}/cancel", job.id),
        Some(&t.operator),
    )
    .send()
    .await
    .unwrap();
    let evs = events(&reader.await.unwrap());
    match &evs.last().unwrap().1 {
        JobEvent::Status { status, error, .. } => {
            assert_eq!(*status, JobStatus::Cancelled);
            assert_eq!(error.as_deref(), Some("cancelled by operator-t"));
        }
        other => panic!("{other:?}"),
    }
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_streams_get_heartbeats_at_least_every_15_seconds() {
    let t = TestServer::start().await;
    let job: Job = t
        .create_job(&t.operator, demo_spec("sleep-hb"))
        .await
        .json()
        .await
        .unwrap();
    t.wait_job(&job.id, 30, |j| j.status == JobStatus::Running)
        .await;
    let resp = t
        .get(&format!("/api/v1/jobs/{}/events", job.id), Some(&t.viewer))
        .send()
        .await
        .unwrap();
    let mut stream = resp.bytes_stream();
    let mut text = String::new();
    let started = std::time::Instant::now();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    while !text.contains("\"type\":\"heartbeat\"") {
        match tokio::time::timeout_at(deadline, stream.next()).await {
            Ok(Some(Ok(chunk))) => text.push_str(&String::from_utf8_lossy(&chunk)),
            other => panic!("no heartbeat within 25 s: {other:?}\n{text}"),
        }
    }
    assert!(started.elapsed() <= Duration::from_secs(20));
    let hb = events(&text)
        .into_iter()
        .find_map(|(f, e)| match e {
            JobEvent::Heartbeat { ts } => Some((f, ts)),
            _ => None,
        })
        .unwrap();
    assert!(hb.0.event.is_none() && hb.0.id.is_none());
    assert!(h2s_daemon::util::parse_rfc3339(&hb.1).is_some());
    drop(stream);
    t.post(
        &format!("/api/v1/jobs/{}/cancel", job.id),
        Some(&t.operator),
    )
    .send()
    .await
    .unwrap();
    t.wait_terminal(&job.id, 30).await;
    t.stop().await;
}
