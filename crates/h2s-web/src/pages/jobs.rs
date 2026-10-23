//! All jobs, newest first, with filters and paging.

use std::time::Duration;

use h2s_api::{Experiment, Job, Page};
use leptos::prelude::*;

use crate::api;
use crate::components::form::SelectField;
use crate::components::ui::{
    Ago, Cursor, EmptyState, ErrorPanel, PageHeader, Pager, Skeleton, StatusBadge,
};
use crate::logic::format;
use crate::logic::jobs::{
    ALL_KINDS, ALL_STATUSES, kind_label, progress_fraction, progress_text, spec_title,
};
use crate::logic::urls::{api as paths, route};
use crate::nav::{query_param, query_setter};

/// The random tail of a job id (UUIDv7 ids share their time-based prefix).
fn short_id(id: &str) -> String {
    let chars: Vec<char> = id.chars().collect();
    chars[chars.len().saturating_sub(8)..].iter().collect()
}

fn duration(job: &Job) -> String {
    match (&job.started_at, &job.finished_at) {
        (Some(s), Some(f)) => format::seconds_between(s, f)
            .map_or_else(|| format::DASH.to_owned(), format::duration_secs),
        (Some(_), None) => "running".to_owned(),
        _ => format::DASH.to_owned(),
    }
}

#[component]
pub(crate) fn Jobs() -> impl IntoView {
    let status = query_param("status");
    let kind = query_param("kind");
    let experiment = query_param("experiment");
    let set_query = query_setter();
    let cursor = Cursor::new();
    let status_sel = RwSignal::new(status.get_untracked());
    let kind_sel = RwSignal::new(kind.get_untracked());
    let exp_sel = RwSignal::new(experiment.get_untracked());
    {
        let set_query = set_query.clone();
        Effect::new(move |prev: Option<()>| {
            let (s, k, e) = (status_sel.get(), kind_sel.get(), exp_sel.get());
            if prev.is_some() {
                set_query(&[("status", &s), ("kind", &k), ("experiment", &e)]);
            }
        });
    }
    Effect::new(move |_| {
        status.track();
        kind.track();
        experiment.track();
        cursor.reset();
    });
    let tick = RwSignal::new(0_u32);
    if let Ok(h) =
        set_interval_with_handle(move || tick.update(|n| *n += 1), Duration::from_secs(5))
    {
        on_cleanup(move || h.clear());
    }
    let page = LocalResource::new(move || {
        tick.track();
        let (s, k, e, c) = (
            status.get(),
            kind.get(),
            experiment.get(),
            cursor.current.get(),
        );
        async move { api::get::<Page<Job>>(&paths::jobs(&s, &k, &e, 25, c.as_deref())).await }
    });
    let experiments = LocalResource::new(|| async {
        api::get::<Vec<Experiment>>(&paths::experiments(true)).await
    });
    let status_options = Signal::derive(|| {
        let mut v = vec![(String::new(), "Any status".to_owned())];
        v.extend(
            ALL_STATUSES
                .iter()
                .map(|s| (s.as_str().to_owned(), s.as_str().to_owned())),
        );
        v
    });
    let kind_options = Signal::derive(|| {
        let mut v = vec![(String::new(), "Any kind".to_owned())];
        v.extend(
            ALL_KINDS
                .iter()
                .map(|k| (k.as_str().to_owned(), kind_label(*k).to_owned())),
        );
        v
    });
    let exp_options = Signal::derive(move || {
        let mut v = vec![(String::new(), "Any experiment".to_owned())];
        if let Some(Ok(list)) = experiments.get() {
            v.extend(list.into_iter().map(|e| (e.name.clone(), e.name)));
        }
        let current = exp_sel.get();
        if !current.is_empty() && !v.iter().any(|(k, _)| *k == current) {
            v.push((current.clone(), current));
        }
        v
    });
    let next = Signal::derive(move || page.get().and_then(Result::ok).and_then(|p| p.next_cursor));
    view! {
        <div class="page">
            <PageHeader title="Jobs" subtitle="Every harness2 operation runs as a job with a live log. Newest first; refreshes every few seconds." />
            <div class="toolbar">
                <SelectField id="jobs-status" label="Status" value=status_sel options=status_options />
                <SelectField id="jobs-kind" label="Kind" value=kind_sel options=kind_options />
                <SelectField id="jobs-experiment" label="Experiment" value=exp_sel options=exp_options />
            </div>
            {move || match page.get() {
                None => view! { <Skeleton lines=8 /> }.into_any(),
                Some(Err(e)) => view! { <ErrorPanel error=e /> }.into_any(),
                Some(Ok(p)) if p.items.is_empty() => view! {
                    <EmptyState
                        title="No jobs here"
                        body="Jobs appear when you run the demo, preview or start a run, set up a benchmark or install harness2. Clear the filters to see everything."
                    >
                        <a class="btn btn-primary" href="/new">"Start a run"</a>
                    </EmptyState>
                }
                .into_any(),
                Some(Ok(p)) => view! {
                    <div class="table-wrap">
                        <table class="table">
                            <thead>
                                <tr>
                                    <th scope="col">"Job"</th>
                                    <th scope="col">"Status"</th>
                                    <th scope="col">"Progress"</th>
                                    <th scope="col">"Experiment"</th>
                                    <th scope="col">"Created"</th>
                                    <th scope="col" class="num">"Duration"</th>
                                    <th scope="col" class="num">"Spend"</th>
                                </tr>
                            </thead>
                            <tbody>
                                {p
                                    .items
                                    .into_iter()
                                    .map(|j| {
                                        let progress = j.progress.as_ref().map(|pr| {
                                            let frac = progress_fraction(pr).map(format::frac_pct).unwrap_or_default();
                                            format!("{} {}", progress_text(pr), if frac.is_empty() { String::new() } else { format!("({frac})") })
                                        });
                                        let exp = j.spec.experiment().map(str::to_owned);
                                        view! {
                                            <tr data-testid="job-row" data-job-id=j.id.clone()>
                                                <td>
                                                    <a class="cell-title" href=route::job(&j.id)>{spec_title(&j.spec)}</a>
                                                    <span class="cell-sub">{format!("{} · …{}", kind_label(j.kind), short_id(&j.id))}</span>
                                                </td>
                                                <td><StatusBadge status=j.status /></td>
                                                <td class="small">{progress.unwrap_or_else(|| format::DASH.to_owned())}</td>
                                                <td>
                                                    {exp.map_or_else(
                                                        || view! { <span class="muted">{format::DASH}</span> }.into_any(),
                                                        |e| view! { <a href=route::experiment(&e)>{e.clone()}</a> }.into_any(),
                                                    )}
                                                </td>
                                                <td class="nowrap">
                                                    <Ago ts=j.created_at.clone() />
                                                    <span class="cell-sub">{j.created_by.clone()}</span>
                                                </td>
                                                <td class="num">{duration(&j)}</td>
                                                <td class="num">{j.spent_usd.map_or_else(|| format::DASH.to_owned(), format::usd)}</td>
                                            </tr>
                                        }
                                    })
                                    .collect_view()}
                            </tbody>
                        </table>
                    </div>
                }
                .into_any(),
            }}
            <Pager cursor=cursor next=next />
        </div>
    }
}
