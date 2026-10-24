//! The MCP server: 41 tools, 3 resource templates and 3 prompts over the Workbench REST API.
//!
//! Each tool is one REST capability (`docs/ARCHITECTURE.md` §8, §13 and §16) called with the
//! caller's token. The server enforces roles and audits; this handler only translates.

use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};

use h2w_api::{
    AuditSession, BenchmarkStatus, Comparison, CostEstimate, DoctorReport, Experiment,
    ExportBundle, FileContent, FileEntry, HarnessDiff, HarnessTree, Job, JobResult, JobSpec,
    JobStatus, LibraryDomain, LogChunk, Manifest, ModelProfile, Page, Principal, ProfileCheck,
    Role, ScoreReport, Scores, SplitInfo, SystemInfo, TaskDetail, TaskRow, ValidationReport,
};
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    handler::server::router::tool::ToolRouter,
    model::{
        CacheScope, GetPromptRequestParams, GetPromptResponse, Implementation, ListPromptsResult,
        ListResourceTemplatesResult, ListResourcesResult, PaginatedRequestParams, ProtocolVersion,
        ReadResourceRequestParams, ReadResourceResponse, ServerCapabilities, ServerConfig,
    },
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use serde::{Serialize, de::DeserializeOwned};

use crate::{
    client::{ApiClient, ApiError},
    descriptions as d, errors,
    limits::WaitLimiter,
    output,
    params::{
        AuditSessionInput, BridgeInput, CheckInput, CompareInput, CreateExperimentInput, DemoInput,
        DiffInput, DoctorInput, EstimateInput, ExperimentInput, ExportInput, HarnessInput,
        InstallInput, JobInput, JobLogInput, JudgeInput, ListExperimentsInput, ListJobsInput,
        ListTasksInput, MAX_WAIT_SECS, NoArgs, PipelineInput, PreviewInput, ProfileInput,
        ReportInput, RolloutFileInput, RolloutFilesInput, RunInput, SetupInput, SplitInput,
        SweepInput, TaskInput, UpdateExperimentInput, ValidateInput, WaitInput,
    },
    prompts, resources,
    schema::{Args, input_schema},
    token::Token,
    wait::{self, ProgressReporter, WaitEnd, WaitOutcome},
};

/// Where the API token for a call comes from.
#[derive(Clone, Debug)]
pub enum Credentials {
    /// stdio: the token the server was started with.
    Fixed(Token),
    /// streamable HTTP: the bearer token of the HTTP request that carried the call.
    PerRequest,
}

/// The Harness² Workbench MCP server.
#[derive(Clone)]
pub struct H2sMcp {
    api: ApiClient,
    credentials: Credentials,
    waits: WaitLimiter,
    allow_risk_acknowledgement: bool,
    tools: Arc<ToolRouter<H2sMcp>>,
}

impl std::fmt::Debug for H2sMcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("H2sMcp")
            .field("api", &self.api)
            .field("credentials", &self.credentials)
            .field("waits", &self.waits)
            .field(
                "allow_risk_acknowledgement",
                &self.allow_risk_acknowledgement,
            )
            .finish_non_exhaustive()
    }
}

/// What `server_info` returns.
#[derive(Serialize)]
struct ServerInfoOutput {
    mcp_server: McpServerVersion,
    server_url: String,
    principal: Principal,
    system: SystemInfo,
}

#[derive(Serialize)]
struct McpServerVersion {
    name: &'static str,
    version: &'static str,
    policy: McpPolicy,
}

/// What this MCP server lets callers do beyond their role.
#[derive(Serialize)]
struct McpPolicy {
    /// `acknowledge_risks=true` is passed on (`--allow-risk-acknowledgement`).
    allow_risk_acknowledgement: bool,
    /// Concurrent waits allowed per API token.
    max_waits_per_token: usize,
}

/// Why `acknowledge_risks=true` is refused unless a human enabled it.
const RISK_ACK_REFUSED: &str = "forbidden: this MCP server does not let a model set \
acknowledge_risks=true, which accepts for the user that the executor runs with the server \
user's files and network (host isolation). A human can start this run from the Workbench web UI; \
an admin can pre-acknowledge host risks in the server configuration; or the person who runs \
h2workbench-mcp can start it with --allow-risk-acknowledgement (H2W_MCP_ALLOW_RISK_ACK=1). Without \
acknowledge_risks the call goes through and the Workbench server decides.";

const MCP_NAME: &str = "h2workbench-mcp";
const MCP_VERSION: &str = env!("CARGO_PKG_VERSION");

const ADVICE_PAGE: &str = "Ask for a smaller page (limit) or filter the list.";
const ADVICE_DETAIL: &str = "Fetch narrower data (a single label, a path, or a diff).";
const ADVICE_JOB: &str = "get_job_log pages through the job's log.";

impl H2sMcp {
    /// A handler that calls the server through `api` with `credentials`.
    #[must_use]
    pub fn new(api: ApiClient, credentials: Credentials) -> Self {
        static TOOLS: OnceLock<Arc<ToolRouter<H2sMcp>>> = OnceLock::new();
        let tools = TOOLS.get_or_init(|| Arc::new(Self::tool_router())).clone();
        Self {
            api,
            credentials,
            waits: WaitLimiter::default(),
            allow_risk_acknowledgement: false,
            tools,
        }
    }

    /// Pass `acknowledge_risks=true` on to the server (off by default: a model must not accept
    /// host-isolation risks for the user unless a human decided so when starting this server).
    #[must_use]
    pub fn with_risk_acknowledgement(mut self, allow: bool) -> Self {
        self.allow_risk_acknowledgement = allow;
        self
    }

    /// Use `waits` to cap concurrent waits per token (share one limiter between the handlers
    /// of a server, as the HTTP transport does; the default allows 4 per token).
    #[must_use]
    pub fn with_wait_limiter(mut self, waits: WaitLimiter) -> Self {
        self.waits = waits;
        self
    }

    /// The token for this call (stdio: the configured one; HTTP: the caller's).
    pub(crate) fn token(&self, ctx: &RequestContext<RoleServer>) -> Result<Token, String> {
        match &self.credentials {
            Credentials::Fixed(token) => Ok(token.clone()),
            Credentials::PerRequest => ctx
                .extensions
                .get::<http::request::Parts>()
                .and_then(|parts| crate::http::bearer_token(&parts.headers))
                .ok_or_else(|| {
                    "unauthorized: this request carries no valid `Authorization: Bearer h2w_...` \
                     header"
                        .to_owned()
                }),
        }
    }

    pub(crate) fn api(&self) -> &ApiClient {
        &self.api
    }

    /// `GET` a route and return its JSON, or the server's error.
    async fn get_json<T: DeserializeOwned + Serialize>(
        &self,
        ctx: &RequestContext<RoleServer>,
        segments: &[&str],
        query: &[(&str, String)],
        needs: Role,
        advice: &str,
    ) -> rmcp::model::CallToolResult {
        let token = match self.token(ctx) {
            Ok(token) => token,
            Err(e) => return output::error(e),
        };
        match self.api.get::<T>(&token, segments, query).await {
            Ok(value) => output::json(&value, None, advice),
            Err(e) => output::error(errors::render(&e, needs)),
        }
    }

    /// `POST` a body and return the JSON answer, or the server's error.
    async fn post_json<B: Serialize, T: DeserializeOwned + Serialize>(
        &self,
        ctx: &RequestContext<RoleServer>,
        segments: &[&str],
        body: Option<&B>,
        needs: Role,
        advice: &str,
    ) -> rmcp::model::CallToolResult {
        let token = match self.token(ctx) {
            Ok(token) => token,
            Err(e) => return output::error(e),
        };
        match self.api.post::<B, T>(&token, segments, body).await {
            Ok(value) => output::json(&value, None, advice),
            Err(e) => output::error(errors::render(&e, needs)),
        }
    }

    /// `POST /jobs`, then optionally wait up to `wait_secs` for the job to finish.
    async fn start_job(
        &self,
        ctx: &RequestContext<RoleServer>,
        spec: JobSpec,
        wait_secs: u32,
    ) -> rmcp::model::CallToolResult {
        let needs = spec.required_role();
        let token = match self.token(ctx) {
            Ok(token) => token,
            Err(e) => return output::error(e),
        };
        let job: Job = match self.api.post(&token, &["jobs"], Some(&spec)).await {
            Ok(job) => job,
            Err(e) => return output::error(errors::render(&e, needs)),
        };
        if wait_secs == 0 {
            let note = started_note(&job);
            return output::json(&job, Some(note), ADVICE_JOB);
        }
        // The job exists now: at the wait cap, report it rather than fail the call.
        let Some(_slot) = self.waits.try_acquire(&token) else {
            let note = format!(
                "Not waiting: this token already runs {} waits, the most allowed at once. {}",
                self.waits.max_per_token(),
                started_note(&job)
            );
            return output::json(&job, Some(note), ADVICE_JOB);
        };
        let job_id = job.id.clone();
        let fallback = job.clone();
        let mut reporter = reporter(ctx);
        let waited = wait::wait_for_job(
            &self.api,
            &token,
            &job_id,
            Some(job),
            secs(wait_secs.min(MAX_WAIT_SECS)),
            Duration::from_secs(2),
            &mut reporter,
            &ctx.ct,
        )
        .await;
        match waited {
            Ok(outcome) => outcome_result(&outcome),
            Err(e) => {
                let note = format!(
                    "Job {job_id} was created, but waiting for it failed ({}); the job keeps \
                     running. Use get_job or wait_for_job with job_id=\"{job_id}\".",
                    errors::render(&e, Role::Viewer)
                );
                output::json(&fallback, Some(note), ADVICE_JOB)
            }
        }
    }
}

fn secs(n: u32) -> Duration {
    Duration::from_secs(u64::from(n))
}

/// The progress reporter for a call (disabled when the client sent no progress token).
fn reporter(ctx: &RequestContext<RoleServer>) -> ProgressReporter {
    ProgressReporter::new(
        ctx.meta
            .get_progress_token()
            .map(|token| (ctx.peer.clone(), token)),
    )
}

/// The serde name of a wire enum value (`parallel`, `opencode`, `running`, ...).
fn wire<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(s)) => s,
        _ => String::new(),
    }
}

fn started_note(job: &Job) -> String {
    format!(
        "Job {id} ({kind}) is {status}. It runs on the server in the background: follow it with \
         wait_for_job(job_id=\"{id}\"), read its output with get_job_log, or stop it with \
         cancel_job.",
        id = job.id,
        kind = job.kind.as_str(),
        status = job.status.as_str(),
    )
}

fn outcome_result(outcome: &WaitOutcome) -> rmcp::model::CallToolResult {
    let job = &outcome.job;
    let note = match outcome.end {
        WaitEnd::Finished => finished_note(job),
        WaitEnd::TimedOut => format!(
            "Job {id} ({kind}) is still {status} after {secs} s of waiting ({now}). It keeps \
             running on the server; call wait_for_job(job_id=\"{id}\") to keep waiting.",
            id = job.id,
            kind = job.kind.as_str(),
            status = job.status.as_str(),
            secs = outcome.waited.as_secs(),
            now = wait::describe(job),
        ),
        WaitEnd::Cancelled => format!(
            "Stopped waiting because the call was cancelled; job {} keeps running.",
            job.id
        ),
    };
    output::json(job, Some(note), ADVICE_JOB)
}

/// What a finished job means and the usual next step.
fn finished_note(job: &Job) -> String {
    let id = &job.id;
    let kind = job.kind.as_str();
    match job.status {
        JobStatus::Succeeded => {
            let next = match &job.result {
                Some(JobResult::Install(_)) => " Next: doctor, then list_benchmarks.".to_owned(),
                Some(JobResult::Demo(_)) => match job.spec.experiment() {
                    Some(exp) => format!(
                        " Browse it with get_experiment(experiment=\"{exp}\"), then list_tasks \
                         and get_task."
                    ),
                    None => String::new(),
                },
                Some(JobResult::Setup(r)) if r.dry_run => {
                    " This was a dry run: result.plan lists what setup would do.".to_owned()
                }
                Some(JobResult::Setup(_)) => " Next: check_benchmark for the same cell.".into(),
                Some(JobResult::Check(r)) if r.ready => " The benchmark is ready.".into(),
                Some(JobResult::Check(_)) => {
                    " The benchmark is NOT ready: see result.failures.".into()
                }
                Some(JobResult::Preview(r)) => format!(
                    " {} tasks; see result.estimate. Results will appear under sweep \"{}\".",
                    r.total_tasks, r.summary_namespace
                ),
                Some(JobResult::Run(r)) => format!(
                    " Sweep \"{}\": score it with start_judge using the same cell, k, seed and \
                     modes.",
                    r.namespace
                ),
                Some(JobResult::Judge(r)) => format!(
                    " {} rows graded, {} ungraded (ungraded are not zeros). Next: get_report.",
                    r.graded, r.ungraded
                ),
                Some(JobResult::Report(_)) => " result.text holds the report.".into(),
                Some(JobResult::Split(r)) => format!(
                    " Use options.split=train or test with options.split_file=\"{}\".",
                    r.name
                ),
                Some(JobResult::Pipeline(p)) => match pipeline_sweep(p) {
                    Some(ns) => format!(
                        " Read the results with get_report(experiment=\"{}\", sweep=\"{ns}\").",
                        job.spec.experiment().unwrap_or_default()
                    ),
                    None => String::new(),
                },
                Some(JobResult::Bridge(_)) | None => String::new(),
            };
            format!("Job {id} ({kind}) succeeded.{next}")
        }
        JobStatus::Partial => format!(
            "Job {id} ({kind}) finished partial (harness2 exit 2): it completed, but some tasks \
             failed or stayed ungraded. See get_job_log(job_id=\"{id}\", tail=100) and the \
             task statuses (list_tasks)."
        ),
        JobStatus::Failed => format!(
            "Job {id} ({kind}) failed{}. Read get_job_log(job_id=\"{id}\", tail=100) for the \
             details.",
            job.error
                .as_deref()
                .map(|e| format!(": {e}"))
                .unwrap_or_default()
        ),
        JobStatus::Cancelled => format!("Job {id} ({kind}) was cancelled."),
        JobStatus::Queued | JobStatus::Running => started_note(job),
    }
}

/// The sweep id a pipeline's run step produced.
fn pipeline_sweep(result: &h2w_api::PipelineResult) -> Option<&str> {
    result.steps.iter().find_map(|step| match step {
        JobResult::Run(run) => Some(run.namespace.as_str()),
        _ => None,
    })
}

#[tool_router(router = tool_router)]
impl H2sMcp {
    // ---------------------------------------------------------------------------- status

    #[tool(
        name = "server_info",
        title = "Server info and identity",
        description = d::SERVER_INFO,
        input_schema = input_schema::<NoArgs>(),
        annotations(
            title = "Server info and identity",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn server_info(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(_): Args<NoArgs>,
    ) -> rmcp::model::CallToolResult {
        let token = match self.token(&ctx) {
            Ok(token) => token,
            Err(e) => return output::error(e),
        };
        let (principal, system) = tokio::join!(
            self.api.get::<Principal>(&token, &["auth", "me"], &[]),
            self.api.get::<SystemInfo>(&token, &["system", "info"], &[]),
        );
        let (principal, system) = match (principal, system) {
            (Ok(p), Ok(s)) => (p, s),
            (Err(e), _) | (_, Err(e)) => return output::error(errors::render(&e, Role::Viewer)),
        };
        let mut notes = Vec::new();
        if principal.role == Role::Admin {
            notes.push(
                "This session uses an admin token. Agents should hold a viewer token, or an \
                 operator token when they must start jobs: ask the user for one."
                    .to_owned(),
            );
        }
        if !system.harness2.installed {
            notes.push(
                "harness2 is not installed on the server yet: an admin can install it from the \
                 web UI (or with install_harness2)."
                    .to_owned(),
            );
        }
        let note = (!notes.is_empty()).then(|| notes.join(" "));
        let info = ServerInfoOutput {
            mcp_server: McpServerVersion {
                name: MCP_NAME,
                version: MCP_VERSION,
                policy: McpPolicy {
                    allow_risk_acknowledgement: self.allow_risk_acknowledgement,
                    max_waits_per_token: self.waits.max_per_token(),
                },
            },
            server_url: self.api.server_url().to_owned(),
            principal,
            system,
        };
        output::json(&info, note, ADVICE_DETAIL)
    }

    #[tool(
        name = "doctor",
        title = "Environment doctor",
        description = d::DOCTOR,
        input_schema = input_schema::<DoctorInput>(),
        annotations(
            title = "Environment doctor",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn doctor(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<DoctorInput>,
    ) -> rmcp::model::CallToolResult {
        let (query, needs) = if input.refresh {
            (vec![("refresh", "true".to_owned())], Role::Operator)
        } else {
            (vec![], Role::Viewer)
        };
        self.get_json::<DoctorReport>(&ctx, &["system", "doctor"], &query, needs, ADVICE_DETAIL)
            .await
    }

    #[tool(
        name = "list_benchmarks",
        title = "List benchmarks",
        description = d::LIST_BENCHMARKS,
        input_schema = input_schema::<NoArgs>(),
        annotations(
            title = "List benchmarks",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn list_benchmarks(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(_): Args<NoArgs>,
    ) -> rmcp::model::CallToolResult {
        self.get_json::<Vec<BenchmarkStatus>>(&ctx, &["benchmarks"], &[], Role::Viewer, "")
            .await
    }

    #[tool(
        name = "setup_benchmark",
        title = "Set up a benchmark (admin)",
        description = d::SETUP_BENCHMARK,
        input_schema = input_schema::<SetupInput>(),
        annotations(
            title = "Set up a benchmark (admin)",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn setup_benchmark(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<SetupInput>,
    ) -> rmcp::model::CallToolResult {
        self.start_job(&ctx, JobSpec::Setup(input.params), input.wait_secs)
            .await
    }

    #[tool(
        name = "check_benchmark",
        title = "Check benchmark readiness",
        description = d::CHECK_BENCHMARK,
        input_schema = input_schema::<CheckInput>(),
        annotations(
            title = "Check benchmark readiness",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn check_benchmark(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<CheckInput>,
    ) -> rmcp::model::CallToolResult {
        self.start_job(&ctx, JobSpec::Check(input.params), input.wait_secs)
            .await
    }

    #[tool(
        name = "list_profiles",
        title = "List model profiles",
        description = d::LIST_PROFILES,
        input_schema = input_schema::<NoArgs>(),
        annotations(
            title = "List model profiles",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn list_profiles(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(_): Args<NoArgs>,
    ) -> rmcp::model::CallToolResult {
        self.get_json::<Vec<ModelProfile>>(&ctx, &["profiles"], &[], Role::Viewer, "")
            .await
    }

    #[tool(
        name = "check_profile",
        title = "Check a model profile",
        description = d::CHECK_PROFILE,
        input_schema = input_schema::<ProfileInput>(),
        annotations(
            title = "Check a model profile",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn check_profile(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<ProfileInput>,
    ) -> rmcp::model::CallToolResult {
        self.get_json::<ProfileCheck>(
            &ctx,
            &["profiles", &input.profile, "check"],
            &[],
            Role::Viewer,
            "",
        )
        .await
    }

    // ----------------------------------------------------------------------- experiments

    #[tool(
        name = "list_experiments",
        title = "List experiments",
        description = d::LIST_EXPERIMENTS,
        input_schema = input_schema::<ListExperimentsInput>(),
        annotations(
            title = "List experiments",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn list_experiments(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<ListExperimentsInput>,
    ) -> rmcp::model::CallToolResult {
        let query = if input.include_archived {
            vec![("archived", "true".to_owned())]
        } else {
            vec![]
        };
        self.get_json::<Vec<Experiment>>(&ctx, &["experiments"], &query, Role::Viewer, "")
            .await
    }

    #[tool(
        name = "create_experiment",
        title = "Create an experiment",
        description = d::CREATE_EXPERIMENT,
        input_schema = input_schema::<CreateExperimentInput>(),
        annotations(
            title = "Create an experiment",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn create_experiment(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<CreateExperimentInput>,
    ) -> rmcp::model::CallToolResult {
        self.post_json::<_, Experiment>(
            &ctx,
            &["experiments"],
            Some(&input.request),
            Role::Operator,
            "",
        )
        .await
    }

    #[tool(
        name = "get_experiment",
        title = "Get an experiment",
        description = d::GET_EXPERIMENT,
        input_schema = input_schema::<ExperimentInput>(),
        annotations(
            title = "Get an experiment",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_experiment(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<ExperimentInput>,
    ) -> rmcp::model::CallToolResult {
        self.get_json::<Experiment>(
            &ctx,
            &["experiments", &input.experiment],
            &[],
            Role::Viewer,
            "",
        )
        .await
    }

    #[tool(
        name = "update_experiment",
        title = "Update or archive an experiment",
        description = d::UPDATE_EXPERIMENT,
        input_schema = input_schema::<UpdateExperimentInput>(),
        annotations(
            title = "Update or archive an experiment",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn update_experiment(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<UpdateExperimentInput>,
    ) -> rmcp::model::CallToolResult {
        let token = match self.token(&ctx) {
            Ok(token) => token,
            Err(e) => return output::error(e),
        };
        match self
            .api
            .patch::<_, Experiment>(&token, &["experiments", &input.experiment], &input.request)
            .await
        {
            Ok(experiment) => output::json(&experiment, None, ""),
            Err(e) => output::error(errors::render(&e, Role::Operator)),
        }
    }

    #[tool(
        name = "list_splits",
        title = "List train/test splits",
        description = d::LIST_SPLITS,
        input_schema = input_schema::<NoArgs>(),
        annotations(
            title = "List train/test splits",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn list_splits(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(_): Args<NoArgs>,
    ) -> rmcp::model::CallToolResult {
        self.get_json::<Vec<SplitInfo>>(&ctx, &["splits"], &[], Role::Viewer, "")
            .await
    }

    #[tool(
        name = "get_manifest",
        title = "Reproducibility manifest",
        description = d::GET_MANIFEST,
        input_schema = input_schema::<ExperimentInput>(),
        annotations(
            title = "Reproducibility manifest",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_manifest(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<ExperimentInput>,
    ) -> rmcp::model::CallToolResult {
        self.get_json::<Manifest>(
            &ctx,
            &["experiments", &input.experiment, "manifest"],
            &[],
            Role::Viewer,
            ADVICE_DETAIL,
        )
        .await
    }

    // -------------------------------------------------------------------------- planning

    #[tool(
        name = "estimate_cost",
        title = "Estimate sweep cost",
        description = d::ESTIMATE_COST,
        input_schema = input_schema::<EstimateInput>(),
        annotations(
            title = "Estimate sweep cost",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn estimate_cost(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<EstimateInput>,
    ) -> rmcp::model::CallToolResult {
        self.post_json::<_, CostEstimate>(
            &ctx,
            &["estimate"],
            Some(&input.request),
            Role::Viewer,
            "",
        )
        .await
    }

    #[tool(
        name = "preview_run",
        title = "Preview a sweep (dry run)",
        description = d::PREVIEW_RUN,
        input_schema = input_schema::<PreviewInput>(),
        annotations(
            title = "Preview a sweep (dry run)",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn preview_run(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<PreviewInput>,
    ) -> rmcp::model::CallToolResult {
        self.start_job(&ctx, JobSpec::Preview(input.params), input.wait_secs)
            .await
    }

    // ------------------------------------------------------------------------------ jobs

    #[tool(
        name = "start_run",
        title = "Start a refinement sweep (costs money)",
        description = d::START_RUN,
        input_schema = input_schema::<RunInput>(),
        annotations(
            title = "Start a refinement sweep (costs money)",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn start_run(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<RunInput>,
    ) -> rmcp::model::CallToolResult {
        if input.params.acknowledge_risks && !self.allow_risk_acknowledgement {
            return output::error(RISK_ACK_REFUSED.to_owned());
        }
        self.start_job(&ctx, JobSpec::Run(input.params), input.wait_secs)
            .await
    }

    #[tool(
        name = "start_pipeline",
        title = "Start check-run-judge-report pipeline (costs money)",
        description = d::START_PIPELINE,
        input_schema = input_schema::<PipelineInput>(),
        annotations(
            title = "Start check-run-judge-report pipeline (costs money)",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn start_pipeline(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<PipelineInput>,
    ) -> rmcp::model::CallToolResult {
        if input.params.acknowledge_risks && !self.allow_risk_acknowledgement {
            return output::error(RISK_ACK_REFUSED.to_owned());
        }
        self.start_job(&ctx, JobSpec::Pipeline(input.params), input.wait_secs)
            .await
    }

    #[tool(
        name = "run_demo",
        title = "Run the free harness2 demo",
        description = d::RUN_DEMO,
        input_schema = input_schema::<DemoInput>(),
        annotations(
            title = "Run the free harness2 demo",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn run_demo(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<DemoInput>,
    ) -> rmcp::model::CallToolResult {
        self.start_job(&ctx, JobSpec::Demo(input.params), input.wait_secs)
            .await
    }

    #[tool(
        name = "start_judge",
        title = "Judge a sweep (costs money)",
        description = d::START_JUDGE,
        input_schema = input_schema::<JudgeInput>(),
        annotations(
            title = "Judge a sweep (costs money)",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn start_judge(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<JudgeInput>,
    ) -> rmcp::model::CallToolResult {
        self.start_job(&ctx, JobSpec::Judge(input.params), input.wait_secs)
            .await
    }

    #[tool(
        name = "start_report",
        title = "Render harness2-report",
        description = d::START_REPORT,
        input_schema = input_schema::<ReportInput>(),
        annotations(
            title = "Render harness2-report",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn start_report(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<ReportInput>,
    ) -> rmcp::model::CallToolResult {
        self.start_job(&ctx, JobSpec::Report(input.params), input.wait_secs)
            .await
    }

    #[tool(
        name = "create_split",
        title = "Create a train/test split",
        description = d::CREATE_SPLIT,
        input_schema = input_schema::<SplitInput>(),
        annotations(
            title = "Create a train/test split",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn create_split(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<SplitInput>,
    ) -> rmcp::model::CallToolResult {
        self.start_job(&ctx, JobSpec::Split(input.params), input.wait_secs)
            .await
    }

    #[tool(
        name = "install_harness2",
        title = "Install harness2 (admin)",
        description = d::INSTALL_HARNESS2,
        input_schema = input_schema::<InstallInput>(),
        annotations(
            title = "Install harness2 (admin)",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn install_harness2(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<InstallInput>,
    ) -> rmcp::model::CallToolResult {
        self.start_job(
            &ctx,
            JobSpec::InstallHarness2(input.params()),
            input.wait_secs,
        )
        .await
    }

    #[tool(
        name = "start_bridge",
        title = "Start the Codex bridge (admin)",
        description = d::START_BRIDGE,
        input_schema = input_schema::<BridgeInput>(),
        annotations(
            title = "Start the Codex bridge (admin)",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn start_bridge(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<BridgeInput>,
    ) -> rmcp::model::CallToolResult {
        self.start_job(&ctx, JobSpec::Bridge(input.params), input.wait_secs)
            .await
    }

    #[tool(
        name = "list_jobs",
        title = "List jobs",
        description = d::LIST_JOBS,
        input_schema = input_schema::<ListJobsInput>(),
        annotations(
            title = "List jobs",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn list_jobs(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<ListJobsInput>,
    ) -> rmcp::model::CallToolResult {
        let mut query = vec![("limit", input.limit.unwrap_or(10).to_string())];
        if let Some(status) = input.status {
            query.push(("status", status.as_str().to_owned()));
        }
        if let Some(kind) = input.kind {
            query.push(("kind", kind.as_str().to_owned()));
        }
        if let Some(experiment) = input.experiment {
            query.push(("experiment", experiment));
        }
        if let Some(cursor) = input.cursor {
            query.push(("cursor", cursor));
        }
        self.get_json::<Page<Job>>(&ctx, &["jobs"], &query, Role::Viewer, ADVICE_PAGE)
            .await
    }

    #[tool(
        name = "get_job",
        title = "Get a job",
        description = d::GET_JOB,
        input_schema = input_schema::<JobInput>(),
        annotations(
            title = "Get a job",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_job(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<JobInput>,
    ) -> rmcp::model::CallToolResult {
        self.get_json::<Job>(
            &ctx,
            &["jobs", &input.job_id],
            &[],
            Role::Viewer,
            ADVICE_JOB,
        )
        .await
    }

    #[tool(
        name = "wait_for_job",
        title = "Wait for a job",
        description = d::WAIT_FOR_JOB,
        input_schema = input_schema::<WaitInput>(),
        annotations(
            title = "Wait for a job",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn wait_for_job(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<WaitInput>,
    ) -> rmcp::model::CallToolResult {
        let token = match self.token(&ctx) {
            Ok(token) => token,
            Err(e) => return output::error(e),
        };
        let Some(_slot) = self.waits.try_acquire(&token) else {
            return output::error(format!(
                "rate_limited: too many concurrent waits for this token ({} already running, \
                 the most allowed at once). Let one finish, or check the job with get_job \
                 instead of waiting.",
                self.waits.max_per_token()
            ));
        };
        let limit = secs(input.timeout_secs.clamp(1, MAX_WAIT_SECS));
        let poll = secs(input.poll_interval_secs.clamp(1, 30));
        let mut reporter = reporter(&ctx);
        match wait::wait_for_job(
            &self.api,
            &token,
            &input.job_id,
            None,
            limit,
            poll,
            &mut reporter,
            &ctx.ct,
        )
        .await
        {
            Ok(outcome) => outcome_result(&outcome),
            Err(e) => output::error(errors::render(&e, Role::Viewer)),
        }
    }

    #[tool(
        name = "cancel_job",
        title = "Cancel a job",
        description = d::CANCEL_JOB,
        input_schema = input_schema::<JobInput>(),
        annotations(
            title = "Cancel a job",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn cancel_job(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<JobInput>,
    ) -> rmcp::model::CallToolResult {
        self.post_json::<(), Job>(
            &ctx,
            &["jobs", &input.job_id, "cancel"],
            None,
            Role::Operator,
            ADVICE_JOB,
        )
        .await
    }

    #[tool(
        name = "get_job_log",
        title = "Read a job's log",
        description = d::GET_JOB_LOG,
        input_schema = input_schema::<JobLogInput>(),
        annotations(
            title = "Read a job's log",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_job_log(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<JobLogInput>,
    ) -> rmcp::model::CallToolResult {
        let token = match self.token(&ctx) {
            Ok(token) => token,
            Err(e) => return output::error(e),
        };
        let (from, limit) = match input.tail {
            Some(tail) => {
                let tail = tail.clamp(1, 5000);
                match self
                    .api
                    .get::<Job>(&token, &["jobs", &input.job_id], &[])
                    .await
                {
                    Ok(job) => (job.log_lines.saturating_sub(u64::from(tail)), tail),
                    Err(e) => return output::error(errors::render(&e, Role::Viewer)),
                }
            }
            None => (input.from, input.limit.unwrap_or(200).clamp(1, 5000)),
        };
        let query = [("from", from.to_string()), ("limit", limit.to_string())];
        match self
            .api
            .get::<LogChunk>(&token, &["jobs", &input.job_id, "log"], &query)
            .await
        {
            Ok(chunk) => output::json(
                &chunk,
                None,
                "Ask for fewer lines (limit) or continue from a later line (from).",
            ),
            Err(e) => output::error(errors::render(&e, Role::Viewer)),
        }
    }

    // --------------------------------------------------------------------------- results

    #[tool(
        name = "list_tasks",
        title = "List a sweep's tasks",
        description = d::LIST_TASKS,
        input_schema = input_schema::<ListTasksInput>(),
        annotations(
            title = "List a sweep's tasks",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn list_tasks(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<ListTasksInput>,
    ) -> rmcp::model::CallToolResult {
        let mut query = vec![("limit", input.limit.unwrap_or(50).to_string())];
        if let Some(domain) = input.domain {
            query.push(("domain", domain));
        }
        if let Some(status) = input.status {
            query.push(("status", status));
        }
        if let Some(cursor) = input.cursor {
            query.push(("cursor", cursor));
        }
        self.get_json::<Page<TaskRow>>(
            &ctx,
            &[
                "experiments",
                &input.experiment,
                "sweeps",
                &input.sweep,
                "tasks",
            ],
            &query,
            Role::Viewer,
            ADVICE_PAGE,
        )
        .await
    }

    #[tool(
        name = "get_task",
        title = "Get a task's record",
        description = d::GET_TASK,
        input_schema = input_schema::<TaskInput>(),
        annotations(
            title = "Get a task's record",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_task(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<TaskInput>,
    ) -> rmcp::model::CallToolResult {
        self.get_json::<TaskDetail>(
            &ctx,
            &[
                "experiments",
                &input.experiment,
                "sweeps",
                &input.sweep,
                "tasks",
                &input.task,
            ],
            &[],
            Role::Viewer,
            ADVICE_DETAIL,
        )
        .await
    }

    #[tool(
        name = "get_harness",
        title = "Get a harness tree",
        description = d::GET_HARNESS,
        input_schema = input_schema::<HarnessInput>(),
        annotations(
            title = "Get a harness tree",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_harness(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<HarnessInput>,
    ) -> rmcp::model::CallToolResult {
        self.get_json::<HarnessTree>(
            &ctx,
            &[
                "experiments",
                &input.experiment,
                "sweeps",
                &input.sweep,
                "tasks",
                &input.task,
                "modes",
                input.mode.as_cli(),
                "harnesses",
                &input.label,
            ],
            &[],
            Role::Viewer,
            "Use diff_harnesses to see only what changed.",
        )
        .await
    }

    #[tool(
        name = "diff_harnesses",
        title = "Diff two harnesses",
        description = d::DIFF_HARNESSES,
        input_schema = input_schema::<DiffInput>(),
        annotations(
            title = "Diff two harnesses",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn diff_harnesses(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<DiffInput>,
    ) -> rmcp::model::CallToolResult {
        self.get_json::<HarnessDiff>(
            &ctx,
            &[
                "experiments",
                &input.experiment,
                "sweeps",
                &input.sweep,
                "tasks",
                &input.task,
                "modes",
                input.mode.as_cli(),
                "diff",
            ],
            &[("from", input.from), ("to", input.to)],
            Role::Viewer,
            ADVICE_DETAIL,
        )
        .await
    }

    #[tool(
        name = "export_harness",
        title = "Export a harness as project files",
        description = d::EXPORT_HARNESS,
        input_schema = input_schema::<ExportInput>(),
        annotations(
            title = "Export a harness as project files",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn export_harness(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<ExportInput>,
    ) -> rmcp::model::CallToolResult {
        self.get_json::<ExportBundle>(
            &ctx,
            &[
                "experiments",
                &input.experiment,
                "sweeps",
                &input.sweep,
                "tasks",
                &input.task,
                "modes",
                input.mode.as_cli(),
                "harnesses",
                &input.label,
                "export",
            ],
            &[
                ("target", wire(&input.target)),
                ("format", "json".to_owned()),
            ],
            Role::Viewer,
            "Export a smaller harness, or read single files with get_harness.",
        )
        .await
    }

    #[tool(
        name = "validate_harness",
        title = "Validate a harness tree",
        description = d::VALIDATE_HARNESS,
        input_schema = input_schema::<ValidateInput>(),
        annotations(
            title = "Validate a harness tree",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn validate_harness(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<ValidateInput>,
    ) -> rmcp::model::CallToolResult {
        self.post_json::<_, ValidationReport>(
            &ctx,
            &["harness", "validate"],
            Some(&input.request),
            Role::Viewer,
            "",
        )
        .await
    }

    #[tool(
        name = "list_rollout_files",
        title = "List a rollout's files",
        description = d::LIST_ROLLOUT_FILES,
        input_schema = input_schema::<RolloutFilesInput>(),
        annotations(
            title = "List a rollout's files",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn list_rollout_files(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<RolloutFilesInput>,
    ) -> rmcp::model::CallToolResult {
        let query = if input.path.is_empty() {
            vec![]
        } else {
            vec![("path", input.path)]
        };
        self.get_json::<Vec<FileEntry>>(
            &ctx,
            &[
                "experiments",
                &input.experiment,
                "sweeps",
                &input.sweep,
                "tasks",
                &input.task,
                "modes",
                input.mode.as_cli(),
                "rollouts",
                &input.label,
                "files",
            ],
            &query,
            Role::Viewer,
            "List a subdirectory (path) instead.",
        )
        .await
    }

    #[tool(
        name = "read_rollout_file",
        title = "Read a rollout file",
        description = d::READ_ROLLOUT_FILE,
        input_schema = input_schema::<RolloutFileInput>(),
        annotations(
            title = "Read a rollout file",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn read_rollout_file(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<RolloutFileInput>,
    ) -> rmcp::model::CallToolResult {
        self.get_json::<FileContent>(
            &ctx,
            &[
                "experiments",
                &input.experiment,
                "sweeps",
                &input.sweep,
                "tasks",
                &input.task,
                "modes",
                input.mode.as_cli(),
                "rollouts",
                &input.label,
                "file",
            ],
            &[("path", input.path)],
            Role::Viewer,
            "The file is long; only its start fits here.",
        )
        .await
    }

    #[tool(
        name = "get_scores",
        title = "Get judge rows",
        description = d::GET_SCORES,
        input_schema = input_schema::<SweepInput>(),
        annotations(
            title = "Get judge rows",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_scores(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<SweepInput>,
    ) -> rmcp::model::CallToolResult {
        self.get_json::<Scores>(
            &ctx,
            &[
                "experiments",
                &input.experiment,
                "sweeps",
                &input.sweep,
                "scores",
            ],
            &[],
            Role::Viewer,
            "Use get_report for aggregates or get_task for one task.",
        )
        .await
    }

    #[tool(
        name = "get_report",
        title = "Get a sweep's score report",
        description = d::GET_REPORT,
        input_schema = input_schema::<SweepInput>(),
        annotations(
            title = "Get a sweep's score report",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_report(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<SweepInput>,
    ) -> rmcp::model::CallToolResult {
        self.get_json::<ScoreReport>(
            &ctx,
            &[
                "experiments",
                &input.experiment,
                "sweeps",
                &input.sweep,
                "report",
            ],
            &[],
            Role::Viewer,
            "",
        )
        .await
    }

    #[tool(
        name = "get_library",
        title = "Get a sweep's experience library",
        description = d::GET_LIBRARY,
        input_schema = input_schema::<SweepInput>(),
        annotations(
            title = "Get a sweep's experience library",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_library(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<SweepInput>,
    ) -> rmcp::model::CallToolResult {
        self.get_json::<Vec<LibraryDomain>>(
            &ctx,
            &[
                "experiments",
                &input.experiment,
                "sweeps",
                &input.sweep,
                "library",
            ],
            &[],
            Role::Viewer,
            "",
        )
        .await
    }

    #[tool(
        name = "list_audit_sessions",
        title = "List improver audit sessions",
        description = d::LIST_AUDIT_SESSIONS,
        input_schema = input_schema::<SweepInput>(),
        annotations(
            title = "List improver audit sessions",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn list_audit_sessions(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<SweepInput>,
    ) -> rmcp::model::CallToolResult {
        self.get_json::<Vec<AuditSession>>(
            &ctx,
            &[
                "experiments",
                &input.experiment,
                "sweeps",
                &input.sweep,
                "audit",
            ],
            &[],
            Role::Viewer,
            "",
        )
        .await
    }

    #[tool(
        name = "read_audit_session",
        title = "Read an audit session",
        description = d::READ_AUDIT_SESSION,
        input_schema = input_schema::<AuditSessionInput>(),
        annotations(
            title = "Read an audit session",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn read_audit_session(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<AuditSessionInput>,
    ) -> rmcp::model::CallToolResult {
        let token = match self.token(&ctx) {
            Ok(token) => token,
            Err(e) => return output::error(e),
        };
        let segments = [
            "experiments",
            &input.experiment,
            "sweeps",
            &input.sweep,
            "audit",
            &input.session,
            input.part.as_str(),
        ];
        match self.api.get_text(&token, &segments, &[]).await {
            Ok(text) => output::text(text, "This part is long; the start is shown above."),
            Err(e) => output::error(errors::render(&e, Role::Viewer)),
        }
    }

    #[tool(
        name = "compare_sweeps",
        title = "Compare two sweeps task by task",
        description = d::COMPARE_SWEEPS,
        input_schema = input_schema::<CompareInput>(),
        annotations(
            title = "Compare two sweeps task by task",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn compare_sweeps(
        &self,
        ctx: RequestContext<RoleServer>,
        Args(input): Args<CompareInput>,
    ) -> rmcp::model::CallToolResult {
        self.get_json::<Comparison>(
            &ctx,
            &["compare"],
            &[("a", input.a.query_value()), ("b", input.b.query_value())],
            Role::Viewer,
            "",
        )
        .await
    }
}

#[tool_handler(router = self.tools)]
impl ServerHandler for H2sMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
                .build(),
        )
        .with_server_info(
            Implementation::new(MCP_NAME, MCP_VERSION)
                .with_title("Harness² Workbench")
                .with_description(
                    "Run and inspect google-research/harness2 agent-harness refinement through \
                     a Harness² Workbench server.",
                )
                .with_website_url(env!("CARGO_PKG_REPOSITORY")),
        )
        .with_instructions(d::INSTRUCTIONS)
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let resources = match self.token(&ctx) {
            Ok(token) => resources::list(self.api(), &token).await,
            Err(_) => Vec::new(),
        };
        let mut result = ListResourcesResult::with_all_items(resources);
        if supports_cache_hints(&ctx) {
            result = result.with_ttl_ms(0).with_cache_scope(CacheScope::Private);
        }
        Ok(result)
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        let mut result = ListResourceTemplatesResult::with_all_items(resources::templates());
        if supports_cache_hints(&ctx) {
            result = result.with_ttl_ms(0).with_cache_scope(CacheScope::Public);
        }
        Ok(result)
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let token = self
            .token(&ctx)
            .map_err(|e| McpError::invalid_request(e, None))?;
        resources::read(self.api(), &token, &request.uri)
            .await
            .map(ReadResourceResponse::Complete)
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        let mut result = ListPromptsResult::with_all_items(prompts::list());
        if supports_cache_hints(&ctx) {
            result = result.with_ttl_ms(0).with_cache_scope(CacheScope::Public);
        }
        Ok(result)
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, McpError> {
        prompts::get(&request.name, request.arguments.as_ref()).map(GetPromptResponse::Complete)
    }
}

/// Cache hints (`ttlMs`, `cacheScope`) exist from protocol version 2026-07-28.
fn supports_cache_hints(ctx: &RequestContext<RoleServer>) -> bool {
    ctx.protocol_version()
        .is_some_and(|version| version >= ProtocolVersion::V_2026_07_28)
}

impl From<ApiError> for McpError {
    fn from(err: ApiError) -> Self {
        let message = errors::render(&err, Role::Viewer);
        match err.status() {
            Some(404) => McpError::resource_not_found(message, None),
            Some(400..=499) => McpError::invalid_request(message, None),
            _ => McpError::internal_error(message, None),
        }
    }
}
