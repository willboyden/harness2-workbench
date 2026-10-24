//! Operations started from several places (dashboard, palette, onboarding, wizard).

use h2s_api::{DemoMode, DemoParams, Job, JobSpec};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::NavigateOptions;

use crate::api;
use crate::logic::urls::{api as paths, route};
use crate::state::{Session, Toasts};

/// Name of the experiment the one-click demo writes into.
pub(crate) const DEMO_EXPERIMENT: &str = "demo";

pub(crate) fn demo_spec() -> JobSpec {
    JobSpec::Demo(DemoParams {
        experiment: DEMO_EXPERIMENT.to_owned(),
        k: 2,
        mode: DemoMode::Both,
    })
}

/// `POST /jobs`, then open the job page. `busy` is true while the request is in flight.
pub(crate) fn start_job(
    spec: JobSpec,
    toasts: Toasts,
    busy: Option<RwSignal<bool>>,
    navigate: impl Fn(&str, NavigateOptions) + 'static,
) {
    if let Some(b) = busy {
        b.set(true);
    }
    spawn_local(async move {
        let result = api::post::<_, Job>(&paths::create_job(), &spec).await;
        if let Some(b) = busy {
            b.set(false);
        }
        match result {
            Ok(job) => {
                toasts.success("Job started");
                navigate(&route::job(&job.id), NavigateOptions::default());
            }
            Err(e) => toasts.error("Could not start the job", &e),
        }
    });
}

/// End the session and go to the sign-in page.
pub(crate) fn sign_out(
    session: Session,
    toasts: Toasts,
    navigate: impl Fn(&str, NavigateOptions) + 'static,
) {
    spawn_local(async move {
        if let Err(e) = api::post_no_content(&paths::logout()).await {
            toasts.error("Sign-out failed", &e);
            return;
        }
        session.principal.set(None);
        session.system.set(None);
        navigate("/login", NavigateOptions::default());
    });
}

/// Start a job and wait until it finishes (polling `GET /jobs/{id}`), for short jobs the
/// UI shows inline (previews, checks). Gives up after `max_secs`.
pub(crate) async fn run_and_wait(spec: &JobSpec, max_secs: u64) -> Result<Job, api::ApiError> {
    let job = api::post::<_, Job>(&paths::create_job(), spec).await?;
    let mut waited_ms = 0_u64;
    loop {
        let current = api::get::<Job>(&paths::job(&job.id)).await?;
        if current.status.is_terminal() || waited_ms >= max_secs * 1000 {
            return Ok(current);
        }
        crate::browser::sleep(800).await;
        waited_ms += 800;
    }
}
