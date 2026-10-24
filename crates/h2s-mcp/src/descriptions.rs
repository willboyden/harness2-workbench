//! Tool descriptions: what each tool does, when to use it, what it costs and what comes back.
//! Kept under 1,024 characters each (some clients cut longer descriptions).

pub(crate) const SERVER_INFO: &str = "Check the connection to the Harness² Studio server and who \
you are. Returns this MCP server's version and policy (whether it accepts acknowledge_risks, \
waits allowed per token), the server URL, your principal (name and role: viewer < operator < \
admin) and the server's system info: Studio version, whether harness2 is installed (commit vs \
the tested commit), the isolation new jobs use (host or container), auth mode, running/maximum \
jobs and features. Call it first. Roles: viewer may use the read-only tools; operator also \
creates experiments and starts or cancels jobs; admin also runs install_harness2, \
setup_benchmark and start_bridge.";

pub(crate) const DOCTOR: &str = "Run the environment doctor: one check per prerequisite (uv, \
Python 3.12, Node 20, git, Docker, Pandoc, Poppler, gcloud and Application Default \
Credentials, harness2, container runtime and runner image, disk) with level ok, warn, fail or \
skip, a detail line, a remedy for the server's host and the features that need it (demo, lab, \
jb, wb, codex, vertex...). ok is true when no check fails. Results are cached for 60 s; \
refresh=true re-runs the probes (operator role). Use it before setting up a benchmark or a \
run, and when a job fails for environmental reasons.";

pub(crate) const LIST_BENCHMARKS: &str = "List the benchmarks harness2 supports and their state \
on the server: JobBench (jb, professional tasks by occupation), LAB (lab, legal tasks by \
practice area; needs Pandoc and Poppler) and WorkBuddy-Bench (wb, one entry per subset \
office/web/code; needs Docker; custom Tencent licence with a territorial limitation). Each \
entry: installed, at_pinned_commit, data_present, domains with task counts (from the last \
preview), requirements, upstream URL, licence note and what it grades. A benchmark must be \
installed (setup_benchmark, admin) and pass check_benchmark before preview_run or start_run.";

pub(crate) const SETUP_BENCHMARK: &str = "Install a benchmark on the server (harness2-setup): \
clone its pinned upstream code, apply harness2's patches, build its environments and download \
task data. Admin role. Network-heavy and slow (minutes; WorkBuddy pulls Docker images), but no \
model calls and no model spend. Use dry_run=true first to see the exact plan. For wb, omitting \
subset installs all three subsets; profile bakes a model profile's settings into WorkBuddy \
routes and Codex config. Starts a background job and returns it at once (wait_secs=0); follow \
it with wait_for_job, then run check_benchmark.";

pub(crate) const CHECK_BENCHMARK: &str = "Run harness2-check for a benchmark cell (bench, \
subset for wb, substrate oc=OpenCode or cx=Codex): offline readiness checks of the installed \
benchmark, its task data, the executor and the profile's model settings. No model calls, no \
cost. Operator role. Waits up to wait_secs (default 45) and returns the job; result.ready, \
result.tasks (installed task count) and result.failures (each FAIL line) tell you what to fix \
(setup_benchmark, doctor remedies, profile secrets) before preview_run or start_run.";

pub(crate) const LIST_PROFILES: &str = "List the model profiles on the server. A profile says \
which models harness2 uses and how to reach them: solver (executor), improver, judge and Codex \
models, endpoint base URLs (never keys), Vertex project/location, price assumptions for \
estimates, egress hosts, and the secrets it references with set/unset state (values are never \
returned). Runs, judging and pipelines need a profile whose secrets are all set, and an \
experiment only accepts the profile it was created with. Admins manage profiles in the web UI.";

pub(crate) const CHECK_PROFILE: &str = "Check a model profile before starting work with it: \
returns warnings (what the profile cannot do, e.g. no judge model, a judge LAB does not \
recognise, WorkBuddy without Vertex Gemini models) and missing_secrets (referenced secrets with \
no value; jobs with the profile are refused until an admin sets them in the web UI). Read-only; \
never returns secret values.";

pub(crate) const LIST_EXPERIMENTS: &str = "List experiments. An experiment is one harness2 \
results directory holding any number of sweeps; a sweep is one (benchmark cell, k, seed, tag) \
group identified by its summary namespace, e.g. lab-oc-agg1-s7 — the `sweep` argument of the \
result tools. For each: name, kind (standard or demo), description, tags, pinned profile, \
creator, archived flag and sweeps with task counts, ok/partial/failed, improver spend (USD) \
and, once judged, headline scores per mode (score and all_pass in PERCENT 0..100). Set \
include_archived=true to include archived experiments.";

pub(crate) const CREATE_EXPERIMENT: &str = "Create an experiment (operator role): a named \
results directory that runs, judging and reports write into. Name: lowercase letters, digits, \
'-' and '_', starting with a letter or digit, at most 63 characters. Pin the model profile you \
will run with (profile): the server refuses runs with another profile, because harness2 will \
not resume or judge a sweep under different models. kind=demo is for demo output; run_demo \
creates its own experiment. Fails with a conflict if the experiment already exists; an \
unregistered directory of that name on disk is adopted.";

pub(crate) const GET_EXPERIMENT: &str = "Get one experiment and its sweeps (fields as in \
list_experiments; headline scores are PERCENT 0..100). Use a sweep's id (summary namespace) as \
the `sweep` argument of list_tasks, get_task, get_report, get_scores, get_library, \
list_audit_sessions and compare_sweeps.";

pub(crate) const UPDATE_EXPERIMENT: &str = "Change an experiment's description, tags or \
archived flag (operator role); arguments you leave out stay as they are. tags replaces the \
whole tag list. archived=true hides the experiment from list_experiments (include_archived \
shows it again) without deleting anything on disk; archived=false restores it. The pinned \
profile and the results cannot be changed here. Returns the updated experiment.";

pub(crate) const LIST_SPLITS: &str = "List the train/test split files created with \
create_split: name (the value for options.split_file), benchmark and subset, seed, train \
fraction, train and test task counts, and creation time. Pass a split's name as \
options.split_file together with options.split=train or test to preview_run, start_run or \
start_pipeline, to adapt on one part of the tasks and measure on the other.";

pub(crate) const ESTIMATE_COST: &str = "Estimate a sweep's size and cost without starting \
anything: executor rollouts (per task: k base runs shared by the modes plus 3k per mode) and \
improver sessions (3k per task and mode, plus selection and reflection sessions), and a USD \
range when prices are known — from the profile's price assumptions (basis heuristic) or \
measured from this experiment's earlier runs (basis history; pass experiment). Judging is not \
included; read notes for caveats. Viewer role. Take tasks from preview_run (total_tasks). Show \
the estimate to the user before start_run or start_pipeline with cloud models.";

pub(crate) const PREVIEW_RUN: &str = "Preview a refinement sweep without running it (harness2 \
--dry-run): the exact task stream in order, namespaces per mode, the summary namespace (the \
sweep id the results will appear under), workers, domains with task counts, totals and a cost \
estimate. No model calls, no cost, nothing written to the experiment; operator role (it runs \
as a short job). Pass the experiment, cell, options and profile you intend to run with, and \
narrow the scope while exploring (options.max_tasks, options.only_domains). Waits up to \
wait_secs (default 45) and returns the job with result.";

pub(crate) const START_RUN: &str = "Start a refinement sweep (harness2): per task, \
Propose-Probe-Compose recursion edits the agent harness (system prompt, guardrails, memory, \
tool guidance, sub-agents, skills, scripts, plugins), runs the executor and a blind selector \
adopts one harness. CALLS MODELS AND COSTS MONEY with cloud models (per task and mode up to 4k \
rollouts and 3k improver sessions). First run preview_run and estimate_cost and get the user's \
go-ahead; set budget_usd (it caps recorded improver spend; executor spend is not recorded). \
profile must be the experiment's pinned profile. Under host isolation the server needs \
acknowledge_risks=true (the executor gets the server user's files and network); this MCP server \
accepts it only when started with --allow-risk-acknowledgement, otherwise the user starts such \
runs in the web UI. Operator role. Returns the queued job; follow with wait_for_job, then \
start_judge with the same cell, k, seed and modes — or use start_pipeline.";

pub(crate) const START_PIPELINE: &str = "Start the whole workflow as one job: preview, check, \
run, judge (and judge the base executions when judge_bases=true), report; it stops at the \
first step that neither succeeds nor ends partial. Same inputs, COST WARNING and \
acknowledge_risks rule as start_run (executor, improver and judge models are called): get the \
user's go-ahead after \
preview_run and estimate_cost, set budget_usd, and set judge_bases=true so the report can \
compare against the unmodified harness. Operator role. Returns the queued job; follow with \
wait_for_job (pipelines run for a long time), then get_report for the sweep (the run step's \
result.namespace is the sweep id).";

pub(crate) const RUN_DEMO: &str = "Run the harness2 demo (harness2-demo): the real recursion and \
experience-library workflow with deterministic local editors — no model credentials, no \
network, no cost, about a second. Writes into a demo experiment (created if missing). k is the \
recursion budget (1-8, default 2); mode is parallel, sequential or both (default). Operator \
role. Waits up to wait_secs (default 45); the result counts done and failed tasks, executions \
and editor calls. Then browse it like any experiment (get_experiment, list_tasks, get_task, \
diff_harnesses, get_library): a safe way to learn the result layout.";

pub(crate) const START_JUDGE: &str = "Score a finished sweep (harness2-judge): the judge model \
grades the selected run of every task and mode against the benchmark's criteria into \
judged.json, or with bases=true the reference executions on the unmodified harness into \
base_judged.json. Calls the judge model: COSTS MONEY with cloud models. Use exactly the run's \
identity — same experiment, cell (bench, subset, substrate), k, seed and modes — and the \
experiment's profile, or nothing matches. Operator role. Ungraded rows (judge failures, no \
pick) are counted separately and are NOT zeros. Returns the queued job; follow with \
wait_for_job, then get_report.";

pub(crate) const START_REPORT: &str = "Render harness2-report from cached judge rows: \
harness2's publication-format block for one or more recursion budgets (ks) and modes of a \
cell. No model calls. Needs judged results (start_judge) with the same cell, seed and modes. \
Operator role. Waits up to wait_secs (default 45); result.text holds the block verbatim. For \
the same figures as structured data (read-only, viewer role) use get_report.";

pub(crate) const CREATE_SPLIT: &str = "Create a reproducible, domain-stratified train/test \
partition of a benchmark's tasks (scripts/split_tasks.py), stored on the server under name \
(which must not exist yet). Use it to adapt on train and measure on test: pass \
options.split=train or test and options.split_file=<name> to preview_run, start_run or \
start_pipeline. train_fraction is each domain's share in train (between 0 and 1, default \
0.5); seed defaults to 42. Operator role, no model calls. Waits up to wait_secs (default 45); \
the result has the train and test counts.";

pub(crate) const LIST_JOBS: &str = "List jobs, newest first: install_harness2, demo, setup, \
check, preview, run, judge, report, split, bridge and pipeline jobs with status (queued, \
running, succeeded, partial, failed, cancelled), progress, spend so far and errors. Filter by \
status, kind or experiment; page with limit (default 10) and the returned next_cursor. Use it \
to find a job id or to see what is running.";

pub(crate) const GET_JOB: &str = "Get one job without waiting: kind, the spec it was started \
with, status, timestamps, exit code, isolation, steps (pipelines and installs have several), \
task progress, recorded spend (USD), a one-line error, and the parsed result once finished \
(preview stream and estimate, run namespace and counts, judge graded/ungraded counts, report \
text, bridge URL...). partial means harness2 exit 2: it finished, but some tasks failed or \
stayed ungraded. Use wait_for_job to block until a job finishes.";

pub(crate) const WAIT_FOR_JOB: &str = "Wait until a job finishes, up to timeout_secs (default \
50, max 600). Follows the server's job events (polling as a fallback) and sends MCP progress \
notifications with task progress (done/total, current task) when your client supplies a \
progress token. Returns the job (as get_job) once it is succeeded, partial, failed or \
cancelled — or, on timeout, the job as it is now with a note; call again to keep waiting \
(sweeps can take hours). Waiting changes nothing on the server. If your client cuts tool calls \
off after a fixed time (often 60 s), keep timeout_secs below it.";

pub(crate) const CANCEL_JOB: &str = "Cancel a queued or running job (operator role): the server \
sends SIGTERM to the job's processes and SIGKILL after 10 s; work in flight is lost and the job \
ends cancelled. Results already written stay in the experiment. Fails with a conflict if the \
job has already finished. Use it when the user asks, when spend runs past what they agreed to, \
or to stop a bridge.";

pub(crate) const GET_JOB_LOG: &str = "Read a job's log (stdout, stderr and server lines; secret \
values are redacted by the server) one page at a time: from (0-based line, default 0) and \
limit (default 200, max 5000). The result has next (pass it as from to continue) and \
complete (the job finished and every line was returned). tail=N returns the last N lines \
instead — the quickest view of why a job failed.";

pub(crate) const LIST_TASKS: &str = "List a sweep's tasks: task id, slug (the `task` argument of \
get_task), domain, status as harness2 records it (ok, skipped, partial, base-failed, running), \
per mode the outcome (ok, chosen harness label, whether the selector kept the base harness, \
fallbacks, improver USD and, once judged, score as a FRACTION 0..1 of criteria passed and \
all_pass) and error count. Filter by domain or status; page with limit (default 50) and \
next_cursor.";

pub(crate) const GET_TASK: &str = "Everything recorded for one task: the prompt, k and seed, \
base (reference) executions and, per mode, the composed finals, each improvement stage with \
the improver's notes (what it changed and why), fallbacks and USD, the cost ledger, similarity \
of the finals, the selector's decision (chosen label, why, anonymised candidates, evidence, \
lesson), the harness labels on disk (base, inv*/dom* candidates, agg*/c* finals) and judge \
rows (frac is a FRACTION 0..1 of criteria passed; graded=false is a judge failure, not a \
zero). Start here to see why a harness was adopted, then diff_harnesses.";

pub(crate) const GET_HARNESS: &str = "Get one harness tree of a task and mode with every file's \
content: label (base, inv1.., dom1.., agg1.. or c1..), role (base, candidate, final), surface \
(opencode or codex), files classified into the eight components (system_prompt, guardrails, \
memory, tool_guidance, subagents, skills, scripts, plugins), a census (always-on characters vs \
the surface's cap; counts of skills, sub-agents, scripts, plugins, hooks) and harness2's \
fingerprint. Trees can be large; to see what changed, diff_harnesses is shorter.";

pub(crate) const DIFF_HARNESSES: &str = "Diff two harness trees of one task and mode (from -> \
to): per file the component, the change (added, removed, modified, unchanged), a unified diff \
with 3 lines of context (full text for added and removed files), lines added and removed, and \
the change in always-on characters. Typical use: from=base, to=the selector's choice (get_task \
-> modes[].selection.chosen_label) to explain what the recursion changed.";

pub(crate) const EXPORT_HARNESS: &str = "Export a harness as project files: target raw (the tree \
as harness2 stores it), opencode (AGENTS.md with the always-on text, .opencode/skills, \
.opencode/agent, .opencode/plugin, scripts) or codex (AGENTS.md, .agents/skills, \
.codex/hooks.json, scripts). Returns each file's relative path and content plus notes (e.g. \
the OpenCode version plugins need). Read-only: nothing is written. A harness changes how an \
agent behaves: show the files to the user and get their agreement before writing them into a \
project; validate_harness checks a raw export or an edited tree.";

pub(crate) const VALIDATE_HARNESS: &str = "Validate a harness tree against harness2's own rules \
(the checks applied to improver proposals) without running anything. Pass the surface \
(opencode or codex), every file's relative path and content, and allow_subagents=false for \
deployments that forbid sub-agents (upstream: LAB). Returns ok, harness2's actionable error \
messages, files outside the editable surface (ignored by harness2) and the census (always-on \
characters vs the cap). Use it for hand-edited harnesses or raw exports before using them.";

pub(crate) const LIST_ROLLOUT_FILES: &str = "List files of a rollout — one execution of a \
harness on a task: the agent's working directory and transcripts as harness2 kept them. label \
selects whose execution (base, inv1, dom1, agg1, c1...; see get_task); path is a directory \
inside the rollout (default its root). Entries are files, directories, symlinks (listed, \
never followed) or other, with sizes. Read one with read_rollout_file.";

pub(crate) const READ_ROLLOUT_FILE: &str = "Read one file of a rollout (see \
list_rollout_files): its text (UTF-8, at most 2 MiB; truncated=true when the file is larger), \
size and MIME type; text is absent for binary files. Use it to inspect what the executor \
produced (deliverables, transcripts) when explaining why a harness did well or badly.";

pub(crate) const GET_SCORES: &str = "Get a sweep's judge rows: rows from judged.json (the \
selected run per task and mode: n_passed/n_criteria, frac = FRACTION 0..1 of criteria passed, \
all_pass, graded, error, metric unit) and base_rows from base_judged.json (reference \
executions on the unmodified harness, after start_judge with bases=true). graded=false rows \
are judge failures or missing picks: they are left out of averages and are NOT zeros. For \
aggregated figures use get_report.";

pub(crate) const GET_REPORT: &str = "Get a sweep's score report computed from its judge rows \
(read-only): per mode and k, graded count n and ungraded count (left out, not zero), mean \
partial-credit score and all-criteria pass rate in PERCENT 0..100, how often the selector kept \
the base harness or chose a final or a candidate, fallbacks, improver USD, metric units; \
base_score (PERCENT 0..100) when base executions were judged; upstream's NOTE and WARNING \
lines (noise floor at k=1, base picks, mixed units) — pass these caveats on; and text in \
harness2-report's format. Needs judged results (start_judge or start_pipeline).";

pub(crate) const GET_LIBRARY: &str = "Get a sweep's experience library: per mode and domain, \
the harness adopted for each task in stream order (task slug, adopted label, run id, a \
one-line component census, the selector's why) and the reflector's lesson (cue, lesson, \
evidence, confidence, per-edit account). Later tasks in a domain read the entries before \
them, so this is what the recursion learned and carried forward. Use it to explain results or \
to choose a harness to adopt for a domain.";

pub(crate) const LIST_AUDIT_SESSIONS: &str = "List the improver and selector sessions in a \
sweep's audit trail: session id, harness label, ok, channel, model, effort, attempts, wall \
time (s), cost (USD), error, and which text parts exist (mission, system, response). Read one \
with read_audit_session — useful when a stage fell back or a proposal was rejected.";

pub(crate) const READ_AUDIT_SESSION: &str = "Read one part of an audit session (see \
list_audit_sessions) as plain text: mission (the brief the improver or selector was given), \
system (its system prompt) or response (what it answered, including proposed edits).";

pub(crate) const COMPARE_SWEEPS: &str = "Compare two (experiment, sweep, mode) results task by \
task, e.g. two seeds, two k values, a harness change or train vs test: per task the scores a \
and b and delta = b - a, all FRACTIONS 0..1 (times 100 = percentage points); means over the \
tasks graded on both sides (n_common), wins, losses, ties, and a two-sided sign-test p-value \
over wins vs losses (ties dropped). A small n_common or a large p-value means the difference \
may be noise.";

pub(crate) const GET_MANIFEST: &str = "Get an experiment's reproducibility manifest: Studio \
version, harness2 commit, dependency pins (benchmark checkouts, OpenCode and Codex versions), \
the profile's model ids, the names (never values) of environment variables passed to \
harness2, isolation and network policy, every job that touched the experiment with its exact \
argv (secret values replaced), each sweep's run identity (config.json) and a SHA-256 digest of \
all summaries and judge results. Use it to report how results were produced or to reproduce \
them.";

pub(crate) const INSTALL_HARNESS2: &str = "Install or reinstall harness2 on the server (admin \
role): fetch google-research/harness2 at the commit this Studio release was tested against \
(MCP clients cannot choose another), create its Python 3.12 environment and install it with extras \
(render is always installed; bridge adds the LiteLLM proxy start_bridge needs). Needs network \
access and takes minutes, no model spend. reinstall=true recreates the environment. Most job \
tools answer `unavailable` until this succeeds. Returns the queued job; follow with \
wait_for_job, then run doctor.";

pub(crate) const START_BRIDGE: &str = "Start harness2-bridge (admin role): a local \
Responses-API endpoint on Vertex AI for the Codex executor (substrate cx). It runs until \
cancelled with cancel_job. Its base URL appears in the job's result (base_url, and \
container_url with expose_to_containers=true so WorkBuddy's containers can reach it); its key \
is stored as the server secret HARNESS2_CODEX_API_KEY and never returned. model defaults to \
the profile's Codex model. Model calls through the bridge cost money. Returns the job; poll \
get_job until it is running and result.base_url is set.";

/// Server instructions sent at initialization.
pub(crate) const INSTRUCTIONS: &str = "Harness² Studio drives google-research/harness2, which \
improves an AI agent's harness (system prompt, guardrails, memory, tool guidance, sub-agents, \
skills, scripts, plugins) by Propose-Probe-Compose recursion on benchmarks (JobBench, LAB, \
WorkBuddy-Bench), then judges and reports. This MCP server is a thin client of a Harness² \
Studio server: every call runs with your API token's role (viewer < operator < admin) and is \
audited by that server.

Tool families: status (server_info, doctor, list_benchmarks, list_profiles, check_profile); \
experiments (list_experiments, create_experiment, get_experiment, update_experiment, \
get_manifest); \
planning (preview_run, estimate_cost, list_splits); jobs (run_demo, check_benchmark, \
start_pipeline, start_run, start_judge, start_report, create_split, setup_benchmark, \
install_harness2, start_bridge, list_jobs, \
get_job, wait_for_job, get_job_log, cancel_job); results (list_tasks, get_task, get_harness, \
diff_harnesses, list_rollout_files, read_rollout_file, get_scores, get_report, get_library, \
list_audit_sessions, read_audit_session, compare_sweeps); adoption (export_harness, \
validate_harness).

Policy: acknowledge_risks=true is accepted only when this MCP server runs with \
--allow-risk-acknowledgement (see server_info); otherwise the user starts host-isolation runs in \
the web UI.

Safe order: server_info -> doctor -> list_benchmarks / check_benchmark -> preview_run -> \
estimate_cost -> the user's go-ahead -> start_pipeline (or start_run then start_judge) with \
budget_usd -> wait_for_job -> get_report -> get_task / diff_harnesses -> export_harness. \
run_demo is free and shows the result layout. start_run, start_pipeline, start_judge and \
start_bridge call models and can cost money. Scores: PERCENT in reports and headlines, \
FRACTIONS 0..1 in judge rows, task rows and comparisons; ungraded is not zero. Prompts: \
improve_agent_harness, analyze_sweep, adopt_best_harness.";
