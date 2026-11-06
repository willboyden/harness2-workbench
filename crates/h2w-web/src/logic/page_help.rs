//! Per-page help: a short lead, numbered *Getting started* steps and a page FAQ for every
//! routed page, shown in each page's help panel.
//!
//! Text is plain text: `backticks` mark inline code, a blank line starts a new paragraph, and
//! nothing is ever parsed as HTML. A step or answer may carry a command (shown with a Copy
//! button, never run) and a link to another page of the app. The tests prove that every
//! route in `app.rs` and every settings section has an entry, and that every link points at a
//! route the router serves.

/// A link to another page of the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Link {
    pub(crate) label: &'static str,
    /// An in-app path starting with `/`, optionally with a `#fragment`.
    pub(crate) href: &'static str,
}

/// One *Getting started* step.
#[derive(Debug)]
pub(crate) struct Step {
    pub(crate) title: &'static str,
    pub(crate) body: &'static str,
    /// A command to copy (never run by the app).
    pub(crate) cmd: Option<&'static str>,
    pub(crate) link: Option<Link>,
}

/// One question of a page's FAQ.
#[derive(Debug)]
pub(crate) struct Faq {
    pub(crate) q: &'static str,
    pub(crate) a: &'static str,
    /// A command to copy (never run by the app).
    pub(crate) cmd: Option<&'static str>,
    pub(crate) link: Option<Link>,
}

/// The help of one page.
#[derive(Debug)]
pub(crate) struct PageHelp {
    pub(crate) key: &'static str,
    /// The page's display name.
    pub(crate) name: &'static str,
    pub(crate) lead: &'static str,
    pub(crate) steps: &'static [Step],
    pub(crate) faq: &'static [Faq],
}

const fn link(label: &'static str, href: &'static str) -> Option<Link> {
    Some(Link { label, href })
}

const fn step(title: &'static str, body: &'static str) -> Step {
    Step {
        title,
        body,
        cmd: None,
        link: None,
    }
}

const fn step_link(title: &'static str, body: &'static str, link: Option<Link>) -> Step {
    Step {
        title,
        body,
        cmd: None,
        link,
    }
}

const fn step_cmd(title: &'static str, body: &'static str, cmd: &'static str) -> Step {
    Step {
        title,
        body,
        cmd: Some(cmd),
        link: None,
    }
}

const fn faq(q: &'static str, a: &'static str) -> Faq {
    Faq {
        q,
        a,
        cmd: None,
        link: None,
    }
}

const fn faq_link(q: &'static str, a: &'static str, link: Option<Link>) -> Faq {
    Faq {
        q,
        a,
        cmd: None,
        link,
    }
}

const fn faq_cmd(q: &'static str, a: &'static str, cmd: &'static str) -> Faq {
    Faq {
        q,
        a,
        cmd: Some(cmd),
        link: None,
    }
}

/// The help of every page, in navigation order.
pub(crate) const PAGES: &[PageHelp] = &[
    PageHelp {
        key: "login",
        name: "Sign in",
        lead: "Sign in with an API token. The page exchanges it for a session cookie that scripts on the page cannot read; the token itself is not kept in the browser.",
        steps: &[
            step_cmd(
                "Get a token",
                "The first administrator token is printed once by `h2workbench init` and saved to `admin.token` next to the configuration file. Administrators create more tokens, one per person or tool, under Settings · API tokens or on the command line.",
                "h2workbench token create --name my-laptop --role operator --expires-in-days 90",
            ),
            step(
                "Paste it and sign in",
                "Paste the whole token (it starts with `h2w_`) into API token and press Sign in. Show reveals what you pasted, so you can check it was copied completely.",
            ),
            step(
                "Land where you were going",
                "A link you opened while signed out brings you here with `?next=`; after signing in you arrive at that page. Only paths inside the app are followed; anything else leads to the dashboard.",
            ),
        ],
        faq: &[
            faq(
                "The server did not accept my token. Why?",
                "The token is unknown, revoked or expired, or it was not copied completely. The server answers all of these with the same message on purpose. Ask an administrator for a new token if you cannot find the problem.",
            ),
            faq(
                "It says “Too many sign-in attempts from this address”.",
                "Sign-in is rate-limited per client address (`server.login_attempts_per_minute`, 10 by default). Wait a minute and try again.",
            ),
            faq(
                "How long does a session last?",
                "`auth.session_ttl_hours` (12 hours by default) and never longer than the token. When it ends, the next thing you do brings you back here; sign in again and you return to where you were.",
            ),
            faq(
                "What decides what I can do?",
                "The token's role: viewer reads everything except secrets, tokens and the audit log; operator also creates experiments and starts or cancels jobs; admin also installs, sets up benchmarks and manages profiles, secrets, tokens and the audit log. The server enforces the role on every request.",
            ),
            faq(
                "I lost my token. Can it be shown again?",
                "No. Tokens are shown once and the server keeps only a hash. An administrator revokes the old one under Settings · API tokens and creates a new one.",
            ),
        ],
    },
    PageHelp {
        key: "dashboard",
        name: "Dashboard",
        lead: "The dashboard answers “is my lab healthy?” at a glance: experiment and sweep counts, running jobs, recorded spend, the harness2 installation and the environment checks.",
        steps: &[
            step(
                "Check the environment",
                "The Environment checks card counts the doctor's results and prints each problem with its fix. Operators and administrators can press Re-run checks after fixing something.",
            ),
            step_link(
                "Make sure harness2 is installed",
                "The harness2 card shows whether it is installed, its version and commit, and the commit this Workbench was tested with. An administrator installs it from Settings · System.",
                link("Open Settings · System", "/settings/system"),
            ),
            step(
                "Try the free demo",
                "Run the demo (operators) starts `harness2-demo` into the experiment `demo` and opens the job. It calls no model and costs nothing, and its results can be explored like a real experiment.",
            ),
            step_link(
                "Start a real run",
                "New run opens the wizard, which previews the exact tasks and estimates the cost before anything starts.",
                link("New run", "/new"),
            ),
        ],
        faq: &[
            faq_link(
                "Why is recorded spend lower than my provider's bill?",
                "harness2 records improver and selection spend only. Executor rollouts are not recorded, so treat the figure as a lower bound.",
                link("Recorded spend", "/help#term-spend"),
            ),
            faq_cmd(
                "An environment check failed. What now?",
                "Each failed or warning row prints a concrete fix. Apply it on the server, then press Re-run checks. The same checks run on the command line.",
                "h2workbench doctor",
            ),
            faq_link(
                "Where did the “New to harness2?” card go?",
                "It shows while there are no experiments or harness2 is not installed, and Dismiss hides it in this browser. The guided tour is always under Getting started in the sidebar.",
                link("Open the guided tour", "/welcome"),
            ),
            faq(
                "Why can I not see Run the demo or New run?",
                "They are operator actions, and your token's role is viewer. The server would refuse them anyway; ask an administrator for an operator token if you need to start jobs.",
            ),
            faq(
                "Does the dashboard update by itself?",
                "Running, queued and recently finished jobs refresh every few seconds. Open a job for its live log and progress.",
            ),
        ],
    },
    PageHelp {
        key: "welcome",
        name: "Getting started",
        lead: "The guided tour explains harness2 in two minutes, then walks the steps from an empty server to a first small run. Each step turns green when it is done.",
        steps: &[
            step(
                "Read how harness2 works",
                "The top of the page explains the two levels of recursion and the eight parts of a harness. Dotted underlines lead to the glossary.",
            ),
            step(
                "Check the server and install harness2",
                "The first steps check the environment and install harness2 (an administrator action on Settings · System).",
            ),
            step(
                "Run the free demo",
                "The demo runs the real recursion with deterministic local editors instead of models: no credentials, no network, about a second. Explore the demo results opens what it wrote.",
            ),
            step(
                "Add a profile, set up a benchmark, start a smoke test",
                "A model profile says which models harness2 uses; a benchmark must be set up before it can run; the smoke test preset runs one task with k = 1 so you can see the whole cycle cheaply.",
            ),
        ],
        faq: &[
            faq(
                "Why does a step stay grey?",
                "Steps turn green from the server's state: the environment checks pass, harness2 is installed, a demo experiment exists, a profile is saved, a benchmark is set up, and a standard experiment has a sweep. Read the results is a pointer and never turns green. Finish a step (or ask an administrator for the admin-only ones) and come back.",
            ),
            faq_link(
                "What does the smoke test cost?",
                "One domain, one task, k = 1, parallel only: the tour shows the number of executor rollouts and improver sessions. The wizard's estimate step gives a cost range when the profile has prices.",
                link("New run", "/new"),
            ),
            faq_link(
                "What is k?",
                "The recursion budget: per task and mode, 4k executor rollouts and 3k improver sessions. Larger k searches more and costs more.",
                link("Glossary: k", "/help#term-k"),
            ),
            faq_link(
                "Can I come back to the tour later?",
                "Yes. Getting started stays in the sidebar, and the dashboard links to it while the lab is empty.",
                link("Dashboard", "/"),
            ),
        ],
    },
    PageHelp {
        key: "experiments",
        name: "Experiments",
        lead: "An experiment is one harness2 results folder tied to one model profile. This list shows each experiment's kind, pinned profile, sweeps, best result, spend and age.",
        steps: &[
            step(
                "Find an experiment",
                "Filter matches the name, description, tags or profile. Show archived includes archived experiments.",
            ),
            step(
                "Create one (operators)",
                "New experiment asks for a name (lowercase letters, digits, `-` and `_`; it is also the folder name and cannot change), a description, tags and optionally a model profile.",
            ),
            step_link(
                "Run something in it",
                "Open the experiment and press New run here, or choose it in the run wizard.",
                link("New run", "/new"),
            ),
        ],
        faq: &[
            faq(
                "Why is the profile pinned for good?",
                "harness2 refuses to resume or judge results under different models, so every run, judging and report in an experiment uses one profile. Without a profile at creation, the first job that writes into the experiment pins it.",
            ),
            faq(
                "Can I rename an experiment?",
                "No: the name is the folder on disk. Create a new experiment instead; Edit changes only the description and tags.",
            ),
            faq(
                "Does archiving delete anything?",
                "No. Archive hides the experiment from lists and pickers; Restore brings it back. Nothing on disk is removed.",
            ),
            faq_link(
                "What is the demo experiment?",
                "The experiment `demo` is written by the free demo. Its kind is `demo` and it has no pinned profile, so it cannot be judged.",
                link("Glossary: experiment", "/help#term-experiment"),
            ),
        ],
    },
    PageHelp {
        key: "experiment",
        name: "Experiment",
        lead: "One experiment: its kind, pinned profile, task count, spend and size, the sweeps it holds and the jobs that touched it.",
        steps: &[
            step(
                "Read the sweeps table",
                "Each row is one run configuration: its id (the summary namespace, such as `demo-oc-agg2`), executor, budget k, seed, modes, tasks, spend and scores. Click a row to open the sweep.",
            ),
            step(
                "Start another run here",
                "New run here (operators) opens the wizard with this experiment chosen and its profile locked.",
            ),
            step(
                "Keep a reproducibility record",
                "Manifest (JSON) downloads the harness2 commit, dependency pins, model ids, environment variable names (never values), isolation, every job's command and a digest of the results. Viewers can download it too.",
            ),
        ],
        faq: &[
            faq(
                "Why does my sweep not appear?",
                "A sweep appears once harness2 writes its first task summary. An experiment lists at most its 200 newest sweeps; older ones still open by their id.",
            ),
            faq(
                "Can I change the pinned profile?",
                "No. harness2 refuses to resume or judge a namespace under different models. Create a new experiment for another profile.",
            ),
            faq_link(
                "What does a sweep id mean?",
                "It is the summary namespace: `<bench>-<executor>-<mode><k>`, optionally `-s<seed>` and a tag. `agg` is parallel mode and `seq` sequential.",
                link("Glossary: namespace", "/help#term-namespace"),
            ),
            faq(
                "What do Edit and Archive change?",
                "Edit changes the description and tags. Archive hides the experiment from lists and pickers without deleting anything; Restore undoes it.",
            ),
        ],
    },
    PageHelp {
        key: "sweep",
        name: "Sweep",
        lead: "A sweep is one benchmark cell, budget k, seed and tag; both recursion modes live under its id. Its tabs show the tasks, the judged report, the experience library and the improver sessions.",
        steps: &[
            step(
                "Scan the tasks",
                "The Tasks tab has one row per task: status, the harness each mode adopted, its score and spend. Filter by domain and status, and click a task to see its whole recursion.",
            ),
            step(
                "Judge the sweep",
                "Scores come from judging, which runs after the sweep. On the Report tab operators press Judge this sweep, and Judge the base runs to score the unmodified harness too.",
            ),
            step(
                "Read the report and the library",
                "Report shows the mean partial credit and all-pass rate per mode, harness2's notes and the report text. Library lists each adopted harness and the lesson stored for later tasks of the domain.",
            ),
            step_link(
                "Compare",
                "Compare… opens the comparison with this sweep chosen; once the base runs are judged, its starting harness is side A.",
                link("Compare", "/compare"),
            ),
        ],
        faq: &[
            faq(
                "Why does the Report tab say “Cannot judge from here”?",
                "The sweep has no benchmark cell, or the experiment has no pinned profile. The demo is like that: it is never judged.",
            ),
            faq_link(
                "A task shows “—” instead of a score.",
                "It was not graded, for example after a judge error. Ungraded tasks are left out of the means, not counted as zero; re-judge before comparing.",
                link("Glossary: ungraded", "/help#term-ungraded"),
            ),
            faq_link(
                "What does “1 fell back” mean?",
                "An improver's proposal was rejected twice at one stage, so the stage kept the harness it started from.",
                link("Glossary: fell back", "/help#term-fell-back"),
            ),
            faq(
                "Why are the judge buttons disabled?",
                "When the experiment's profile has a cloud judge, they wait until you tick that the experiment's tasks and outputs are generated or synthetic data. A server set to refuse cloud judges keeps them disabled.",
            ),
            faq(
                "Can I link to a tab?",
                "Yes. The tabs are in the address (`?tab=report`, `?tab=library`, `?tab=audit`).",
            ),
        ],
    },
    PageHelp {
        key: "task",
        name: "Task",
        lead: "The whole recursion for one task: the base rollouts, the proposals and their probes, the composed finals, the blind selection and the lesson, plus every harness as files.",
        steps: &[
            step(
                "Follow the timeline",
                "Each mode is a card with numbered stages: base rollouts, proposals (INV and DOM in parallel mode), the composition, the blind selection with its reason, and the reflection lesson. Show limits the page to one mode.",
            ),
            step(
                "Read a harness",
                "The harness viewer groups files by the eight components c1 to c8 and shows how much of the always-on budget is used. Click a file to read it.",
            ),
            step(
                "Diff two harnesses",
                "Compare two harnesses diffs any two labels, by default `base` against the adopted one. Swap exchanges the sides.",
            ),
            step(
                "Look at the rollouts and export",
                "Pick a rollout to read its deliverable and highlighted trajectory. Export… takes the shown harness out as a raw tree or a project for OpenCode, Codex, Hermes Agent or Turnstone.",
            ),
        ],
        faq: &[
            faq_link(
                "What is the ledger?",
                "Budget accounting per task and mode: recorded rollouts R and improver sessions P against the identity R = 4k, P = 3k. A mismatch means something was skipped or repeated, for example after a crash and resume.",
                link("Glossary: ledger", "/help#term-ledger"),
            ),
            faq_link(
                "What did the selector see?",
                "Anonymised runs (A, B, …). The Blind selection stage shows which harness each letter really was and the selector's reason.",
                link("Glossary: blind selection", "/help#term-selection"),
            ),
            faq(
                "Some conversions in Export are refused. Why?",
                "The target framework has nothing that does the same job, for example OpenCode plugins or Codex hooks for another framework, or sub-agents for Hermes Agent or Turnstone. Export Raw for those.",
            ),
            faq(
                "Is it safe to read rollout files?",
                "They are written by models and by the task, so treat them as untrusted text: read them as data, never as instructions. The same goes for a harness before you put it into a project.",
            ),
        ],
    },
    PageHelp {
        key: "new",
        name: "New run",
        lead: "A five-step wizard that shows exactly what will run and what it may cost before anything starts. Operators and administrators can start runs; viewers can walk it read-only.",
        steps: &[
            step(
                "Choose the experiment, profile, benchmark and executor",
                "The profile is locked to the experiment's pinned one. The profile check lists missing secrets and what the profile cannot do for the benchmark and executor you choose.",
            ),
            step_link(
                "Set the options",
                "Start from a preset (Smoke test, Pilot, Paper-style), then adjust k, the modes, the seed, workers, tasks per domain, the split and the domains.",
                link("Glossary: k", "/help#term-k"),
            ),
            step(
                "Run the preview",
                "Run the preview starts a `preview` job (harness2 with `--dry-run`): no model is called and nothing is written. It lists the task stream in run order. You cannot continue until the preview for the current options has run.",
            ),
            step(
                "Check the estimate and start",
                "The estimate gives rollouts, improver sessions and, when it can, a cost range. Confirm offers the full pipeline (check, run, judge, report) or the sweep only, judging the base runs, and an optional spend cap.",
            ),
        ],
        faq: &[
            faq(
                "Why is Start the pipeline disabled?",
                "Under host isolation tick “I understand and want to run on the host”, unless the server pre-acknowledges host risks. With a cloud judge, also tick the synthetic-data box. Viewers cannot start runs at all.",
            ),
            faq_link(
                "A benchmark says “Not set up yet”.",
                "An administrator sets it up on the Benchmarks page first. A benchmark the server does not run says why and stops the wizard at the first step.",
                link("Benchmarks", "/benchmarks"),
            ),
            faq(
                "Does the spend cap stop the run immediately?",
                "No. It is checked as task summaries land, so a task in flight finishes first, and it counts recorded improver spend only: executor spend is not recorded.",
            ),
            faq_link(
                "What does the seed change?",
                "It shuffles the task order reproducibly; it does not control model sampling. With a seed, tasks per domain takes the first tasks of the shuffled order.",
                link("Glossary: seed", "/help#term-seed"),
            ),
            faq_link(
                "How do I run on chosen tasks only?",
                "Create a split of chosen tasks on the Benchmarks page, then choose split train and that split file here.",
                link("Benchmarks", "/benchmarks"),
            ),
        ],
    },
    PageHelp {
        key: "jobs",
        name: "Jobs",
        lead: "Every harness2 operation is a job: the demo, a preview, a check, a run, judging, a report, a split, an installation or a benchmark setup. The list is newest first and refreshes every few seconds.",
        steps: &[
            step(
                "Filter the list",
                "Filter by status, kind and experiment. Each row shows the title, kind and short id, status, progress, experiment, who started it and when, duration and spend.",
            ),
            step(
                "Open a job",
                "Click a row for its steps, parsed result, specification and live log.",
            ),
            step_link(
                "Start something",
                "New jobs come from the run wizard, the dashboard's demo button, a sweep's judge buttons and the Benchmarks page.",
                link("New run", "/new"),
            ),
        ],
        faq: &[
            faq(
                "What does “partial” mean?",
                "harness2 finished but some tasks failed or stayed ungraded (it exited with code 2). Re-running the same command resumes and retries them.",
            ),
            faq(
                "Why is a job still queued?",
                "At most `jobs.max_concurrent` jobs run at once (2 by default); the rest wait their turn. The dashboard shows running against allowed.",
            ),
            faq(
                "Who can cancel a job?",
                "Operators and administrators, the roles that can start jobs. The job page asks you to confirm first.",
            ),
            faq(
                "How long are jobs kept?",
                "The job record stays. Its log is deleted `jobs.log_retention_days` after the job finished (90 days by default), so an old job may show no log.",
            ),
        ],
    },
    PageHelp {
        key: "job",
        name: "Job",
        lead: "One job: its status and progress, the steps, the result parsed from harness2's output, the job specification and the log, streamed live while it runs.",
        steps: &[
            step(
                "Watch it run",
                "While running, the status chip says Live and the progress bar, current task and log lines update as they are written.",
            ),
            step(
                "Search the log",
                "Search the log filters lines (case-insensitive); the stdout, stderr and server switches choose the streams; Follow new lines keeps the newest line in view; Download saves what the page holds.",
            ),
            step(
                "Read the result",
                "When the job ends, Result shows what harness2 reported, such as the namespaces or the scores, with links to the sweep it wrote.",
            ),
        ],
        faq: &[
            faq(
                "What happens when I cancel?",
                "harness2's process group gets SIGTERM, then SIGKILL after 10 seconds. Finished tasks are kept, and the task in flight starts over if you resume the sweep.",
            ),
            faq(
                "Why does the log not show its beginning?",
                "The page holds the newest 20,000 lines (fewer when they are long). Show earlier lines fetches older ones; the REST API pages through the whole log.",
            ),
            faq(
                "I see [REDACTED:NAME] in the log.",
                "Secret values are replaced before a line is stored. Redaction is best effort and every viewer can read every job's log, so do not give a job a secret you cannot afford to see in a log.",
            ),
            faq(
                "The log does not update live.",
                "The page streams over server-sent events. A reverse proxy that buffers responses holds them back; an administrator turns buffering off for the events route.",
            ),
            faq(
                "A setup plan says nothing was changed.",
                "That was a dry run (Plan). The job page lists the planned steps and offers Run setup for real, after a confirmation.",
            ),
        ],
    },
    PageHelp {
        key: "compare",
        name: "Compare",
        lead: "Two results side by side, task by task: mean scores, the difference B − A, wins, losses and ties, a sign test and, when it can be measured, the noise floor.",
        steps: &[
            step(
                "Pick A and B",
                "A is the reference and B the challenger: each a sweep and mode, or a sweep's starting harness once its base runs are judged.",
            ),
            step(
                "Read What this means",
                "It puts the numbers into plain words: whether the difference is beyond the noise floor and whether the sign test can be trusted with this many decisive tasks.",
            ),
            step(
                "Share it",
                "The chosen pair is in the address, so the link opens the same comparison. Swap A and B exchanges them.",
            ),
        ],
        faq: &[
            faq_link(
                "Why can the sign test not reach p < 0.05?",
                "With fewer than 6 decisive tasks (ties dropped) no split is lopsided enough. Run more tasks before concluding.",
                link("Glossary: sign test", "/help#term-sign-test"),
            ),
            faq_link(
                "What is the noise floor?",
                "How far the same harness moved between runs on these tasks. A difference within it can be run-to-run variation: repeat with another seed first.",
                link("Glossary: noise floor", "/help#term-noise-floor"),
            ),
            faq(
                "Why are some tasks missing from the means?",
                "Only tasks graded on both sides count. Ungraded tasks are left out of the means and the test.",
            ),
            faq(
                "Why is the starting harness not offered for my sweep?",
                "It appears once the sweep's base runs are judged: press Judge the base runs on the sweep's Report tab.",
            ),
        ],
    },
    PageHelp {
        key: "benchmarks",
        name: "Benchmarks",
        lead: "What harness2 can run on this server: JobBench, LAB and the three WorkBuddy-Bench subsets, each with its setup state, licence and needs, plus your train/test splits.",
        steps: &[
            step(
                "Read the card",
                "Each card says whether the benchmark is set up and has task data, its licence and what it needs. WorkBuddy-Bench's licence has a territorial limitation: read it before you download or use that benchmark.",
            ),
            step(
                "Check readiness (operators)",
                "Check readiness runs harness2's offline readiness check for the benchmark; no model is called.",
            ),
            step(
                "Set it up (administrators)",
                "Set up… opens a dialog: Plan (dry run) prints every command without changing anything; Set up now clones the pinned code, builds environments and downloads task data, which can take long.",
            ),
            step_link(
                "Make a split",
                "New split… makes a random, domain-stratified split, or a split of tasks you choose, so you can improve on one part and evaluate on the other.",
                link("Glossary: split", "/help#term-split"),
            ),
        ],
        faq: &[
            faq(
                "A card says “not on this server”.",
                "An administrator listed the benchmark under `[[benchmarks.not_offered]]`, usually because another Workbench runs it; the note says which, and a link opens it when one was configured.",
            ),
            faq(
                "A card says it needs host isolation.",
                "This server runs jobs in containers, and WorkBuddy-Bench starts a container of its own for every trial, which a job inside a container cannot do. Ask your administrator where it runs.",
            ),
            faq(
                "Why does setup ask for a model profile?",
                "Some benchmarks bake model routes into their environments (WorkBuddy, Codex). For the others the profile is optional.",
            ),
            faq(
                "What format do chosen tasks use?",
                "One task id per line, as the preview lists them: `<domain>/<task>`, or a bare name for WorkBuddy-Bench. Blank lines and `#` comments are ignored.",
            ),
        ],
    },
    PageHelp {
        key: "settings-profiles",
        name: "Model profiles",
        lead: "A model profile says which models harness2 uses (solver, improver, judge, Codex) and how to reach them: Google Vertex AI, any OpenAI-compatible endpoint, or raw environment variables.",
        steps: &[
            step_link(
                "Store the API key first",
                "Keys are never typed into a profile. An administrator stores the key under Settings · Secrets and the profile references it by name.",
                link("Settings · Secrets", "/settings/secrets"),
            ),
            step(
                "Create the profile (administrators)",
                "New profile asks for the kind, the models, the endpoint and the name of the key's secret, plus optional judge and Codex endpoints, extra environment variables, prices and egress hosts.",
            ),
            step(
                "Read the profile check",
                "After saving, a card says what jobs with this profile would still lack, such as a missing secret or a benchmark it cannot serve.",
            ),
            step_link(
                "Use it",
                "Pick the profile when you create an experiment or in the run wizard. An experiment keeps its profile for good.",
                link("New run", "/new"),
            ),
        ],
        faq: &[
            faq_link(
                "What is a cloud judge?",
                "A judge model at a cloud provider. It sees every task and output it grades, so use one only with generated or synthetic data. Judging with it asks you to confirm that.",
                link("Glossary: cloud judge", "/help#term-cloud-judge"),
            ),
            faq(
                "Why does the card say “secret missing”?",
                "The profile references a secret that has no value. Set the missing secret takes an administrator to the Secrets page; jobs with the profile are refused until it is set.",
            ),
            faq(
                "Why can I not delete a profile?",
                "An experiment is pinned to it. Deleting is refused while that is so.",
            ),
            faq(
                "Does the egress hosts box open the network for jobs?",
                "No, it is informational. An administrator adds each host to `[egress] allow` in the server configuration.",
            ),
            faq(
                "Which judge works with which benchmark?",
                "It depends on the benchmark and the provider: LAB judges `claude` and `gemini` ids on Vertex AI, JobBench sends temperature 0, and WorkBuddy-Bench needs Gemini on Vertex AI. The Judge presets explain each choice; run one task first with any new judge.",
            ),
        ],
    },
    PageHelp {
        key: "settings-secrets",
        name: "Secrets",
        lead: "Secrets hold model-provider keys. They are write-only: the page shows a name, whether it is set and its length, never the value. Administrators only.",
        steps: &[
            step(
                "Add a secret",
                "Add or replace a secret takes a name (uppercase letters, digits and underscores) and the value, which is not shown again.",
            ),
            step_link(
                "Reference it from a profile",
                "Model profiles name the secret, never the key itself.",
                link("Model profiles", "/settings/profiles"),
            ),
            step(
                "Fill in what is missing",
                "Secrets a profile references but that have no value are listed with Set value. Stored secrets offer Replace value and Delete.",
            ),
        ],
        faq: &[
            faq(
                "Can I read a secret back?",
                "No. No endpoint ever returns a value; the length is shown so a truncated paste is easy to spot.",
            ),
            faq(
                "What does `H2W_SECRET_<NAME>` do?",
                "An environment variable of the server with that name supplies the secret's value and takes precedence over a stored one. It never touches disk.",
            ),
            faq(
                "Which jobs get a secret?",
                "Only jobs whose profile references it, through the job's environment, never its command line.",
            ),
            faq(
                "What happens when I delete a secret?",
                "Profiles that reference it refuse to run until it is set again.",
            ),
        ],
    },
    PageHelp {
        key: "settings-tokens",
        name: "API tokens",
        lead: "API tokens sign people and tools in, each with a role. Administrators create and revoke them here; a new token is shown once.",
        steps: &[
            step(
                "Create a token",
                "Give it a name that says who or what uses it, the smallest role it needs, and an expiry (90 days is recommended).",
            ),
            step(
                "Copy it once",
                "The new token is shown once. Copy it straight into a password manager or the tool's configuration, then press Done, I saved it.",
            ),
            step(
                "Revoke what is no longer used",
                "Revoke ends the token and its browser sessions at once. Last used helps find stale tokens.",
            ),
        ],
        faq: &[
            faq(
                "Which role should an MCP agent get?",
                "Viewer, unless it must start jobs. Give every person and tool its own token with the smallest role it needs.",
            ),
            faq_cmd(
                "Can I create a token on the command line?",
                "Yes, on the server: the token is the only line printed on standard output.",
                "h2workbench token create --name ci-runner --role viewer --expires-in-days 90",
            ),
            faq(
                "Why is “never” discouraged as expiry?",
                "A token that never expires stays valid if it leaks and is forgotten. Prefer an expiry and create a new token when it runs out.",
            ),
            faq_cmd(
                "I locked myself out of the admin role.",
                "On the server, `init --force` mints another bootstrap admin token without touching the configuration.",
                "h2workbench init --force",
            ),
        ],
    },
    PageHelp {
        key: "settings-audit",
        name: "Audit log",
        lead: "Every sign-in, job start and cancel, and every change to experiments, profiles, secrets and tokens, appended to a SHA-256 hash chain. Values and request bodies are never recorded.",
        steps: &[
            step(
                "Read the entries",
                "Each entry has when, who, the action, the target, the outcome and details. Who is `token:<name>`, `proxy:<user>`, `cli`, `server` or `anonymous`.",
            ),
            step(
                "Verify the chain",
                "Verify the chain re-walks the hash chain and says whether all entries link correctly.",
            ),
            step_cmd(
                "Verify on the server too",
                "The same check runs on the command line and prints the head of the chain. Keep that head hash somewhere the server's user cannot write.",
                "h2workbench audit verify",
            ),
        ],
        faq: &[
            faq(
                "Is the log tamper-proof?",
                "No. It catches accidental or casual edits; anyone who can write the server's data directory can rewrite the whole chain. Comparing with a head hash kept elsewhere is what makes it evidence.",
            ),
            faq(
                "Why are some refused requests missing?",
                "Refused credentials are recorded at most 10 a minute per client address, and refused calls at most 10 a minute per token or proxy user; the rest are only counted, so junk requests cannot flood the log.",
            ),
            faq(
                "Does the log contain secret values?",
                "No. A secret change records its length, never the value.",
            ),
            faq(
                "The log is empty.",
                "Entries appear as people sign in and act. A fresh server has none yet.",
            ),
        ],
    },
    PageHelp {
        key: "settings-system",
        name: "System",
        lead: "The server's version, authentication and isolation modes, jobs, features, the harness2 installation and the environment checks. Administrators also see the data directory, install harness2 here and draft the server configuration.",
        steps: &[
            step(
                "Check the server",
                "Read the version, authentication mode, isolation mode and how many jobs run. The environment checks list each tool with its result and fix; Re-run checks refreshes them (operators and administrators).",
            ),
            step(
                "Install or reinstall harness2 (administrators)",
                "Install or reinstall harness2 installs the commit this Workbench release was tested with. Re-create the virtual environment helps when it is damaged or after a Python upgrade.",
            ),
            step(
                "Load a configuration draft (administrators)",
                "Server configuration loads the server's own `h2workbench.toml` read-only, the annotated example shipped with the Workbench, or a file you paste or open (it is read in the browser and sent nowhere except to Validate).",
            ),
            step(
                "Edit and validate",
                "The Form view (every key with an (i) info button) and the Text view share one draft. Validate runs the server's real parser and validator on the text and lists every problem; it never writes anything.",
            ),
            step_cmd(
                "Export and install it by hand",
                "Export shows a diff against what you loaded, with Copy and Download. Check the new file on the server, copy it over the configured file (mode 0600) and restart the server: the configuration is read only at start.",
                "h2workbench --config ./h2workbench.toml config check",
            ),
        ],
        faq: &[
            faq(
                "Why can the web UI not save the configuration?",
                "On purpose: the file decides authentication, trusted proxies, allowed hosts, isolation and the egress allowlist, so no route of the server writes it. You install a validated draft by hand.",
            ),
            faq(
                "The server refused to send its configuration file.",
                "It refuses when the file, comments included, holds something that looks like a credential, as far as the server can tell: a URL with user information or a query string, a private key, an API token, a long token-like string, or a value given to a name like a password or token (`name = value`, or `name: value` with one word). A file that passes those checks is refused too when it holds the value of a stored secret (8 bytes or more). The answer says which. Remove it from the file on the server (credentials belong in Settings · Secrets, referenced by name), or load the example or your own copy instead.",
            ),
            faq(
                "I changed `[egress]`. What do I restart?",
                "The allowlist is read by the `h2workbench egress-proxy` process, so restart that as well as the server.",
            ),
            faq(
                "Validation passed, but the server uses other values.",
                "Environment overrides win over the file: `H2W_LISTEN`, `H2W_DATA_DIR`, `H2W_PUBLIC_URL`, `H2W_LOG_FORMAT` and `H2W_OTLP_ENDPOINT`. Check the server's environment.",
            ),
            faq(
                "Why does installing another commit fail?",
                "The server installs only the tested harness2 commit unless an administrator sets `paths.allow_untested_harness2_rev = true`. Installing runs the build code of that commit, so use only commits you trust.",
            ),
            faq(
                "Why do I not see the data directory?",
                "Absolute paths on the server are shown to administrators only.",
            ),
        ],
    },
    PageHelp {
        key: "help",
        name: "Help",
        lead: "Every term the Workbench uses, the eight harness components, frequently asked questions and the keyboard shortcuts.",
        steps: &[
            step(
                "Look a term up",
                "Filter terms matches names and definitions. Every term has its own link, and dotted underlines elsewhere in the app lead here.",
            ),
            step(
                "Learn the eight components",
                "The table lists c1 to c8, what each is, and which are always on (read on every task, and capped in size).",
            ),
            step(
                "Learn the shortcuts",
                "Ctrl K (⌘ K on a Mac) opens the command palette; `?` shows the shortcuts; `g` then a letter goes to a page.",
            ),
        ],
        faq: &[
            faq_link(
                "Where are the general questions?",
                "In the Frequently asked questions section of this page. Each page also has its own Getting started and FAQ.",
                link("Frequently asked questions", "/help#faq"),
            ),
            faq_link(
                "Where do I start as a new user?",
                "The guided tour walks from an empty server to a first small run.",
                link("Open the guided tour", "/welcome"),
            ),
            faq(
                "How do I switch between light and dark?",
                "The theme button at the top cycles system, light and dark; the choice is kept in this browser.",
            ),
            faq(
                "Does the app work on a phone?",
                "Yes. On narrow screens the sidebar becomes a drawer behind the menu button and tables scroll inside their container.",
            ),
        ],
    },
];

/// The help with this key.
pub(crate) fn find(key: &str) -> Option<&'static PageHelp> {
    PAGES.iter().find(|p| p.key == key)
}

/// A run of text inside a paragraph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Run<'a> {
    Text(&'a str),
    Code(&'a str),
}

/// Paragraphs (separated by a blank line) of runs; `backticks` mark inline code. Never HTML.
/// A backtick without a partner is kept as text.
pub(crate) fn paragraphs(text: &str) -> Vec<Vec<Run<'_>>> {
    text.split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(runs)
        .collect()
}

fn runs(p: &str) -> Vec<Run<'_>> {
    let mut out = Vec::new();
    let mut rest = p;
    while let Some(open) = rest.find('`') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('`') else {
            break;
        };
        if open > 0 {
            out.push(Run::Text(&rest[..open]));
        }
        out.push(Run::Code(&after[..close]));
        rest = &after[close + 1..];
    }
    if !rest.is_empty() {
        out.push(Run::Text(rest));
    }
    out
}

/// FAQ entries whose question or answer contains every whitespace-separated word of `query`
/// (case-insensitive); an empty query keeps them all.
pub(crate) fn filter_faq<'a>(faq: &'a [Faq], query: &str) -> Vec<&'a Faq> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    faq.iter()
        .filter(|f| {
            let hay = format!("{}\n{}", f.q, f.a).to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .collect()
}

/// Settings sections that have help (`settings-<id>`).
const SETTINGS_KEYS: &[(&str, &str)] = &[
    ("profiles", "settings-profiles"),
    ("secrets", "settings-secrets"),
    ("tokens", "settings-tokens"),
    ("audit", "settings-audit"),
    ("system", "settings-system"),
];

/// Route patterns (as written in `app.rs`) and their help keys.
const ROUTE_KEYS: &[(&str, &str)] = &[
    ("/login", "login"),
    ("/", "dashboard"),
    ("/welcome", "welcome"),
    ("/experiments", "experiments"),
    ("/experiments/:exp", "experiment"),
    ("/experiments/:exp/sweeps/:sweep", "sweep"),
    ("/experiments/:exp/sweeps/:sweep/tasks/:task", "task"),
    ("/new", "new"),
    ("/jobs", "jobs"),
    ("/jobs/:id", "job"),
    ("/compare", "compare"),
    ("/benchmarks", "benchmarks"),
    ("/help", "help"),
];

fn segments(path: &str) -> Vec<&str> {
    path.split('/').filter(|s| !s.is_empty()).collect()
}

/// `true` when `path` (concrete or a pattern) matches `pattern`, whose `:name` segments match
/// any one segment.
fn route_matches(pattern: &str, path: &str) -> bool {
    let (p, s) = (segments(pattern), segments(path));
    p.len() == s.len() && p.iter().zip(&s).all(|(a, b)| a.starts_with(':') || a == b)
}

/// The help key for a route pattern as written in `app.rs` `path!(...)`
/// (`/experiments/:exp` -> `experiment`), or for a concrete path (`/experiments/demo`).
/// Settings sections are `settings-<section>` (`/settings/system`); `/settings` (a redirect)
/// and the bare pattern `/settings/:section` have none.
pub(crate) fn key_for_route(pattern: &str) -> Option<&'static str> {
    let path = pattern.split(['?', '#']).next().unwrap_or(pattern);
    if let Some(section) = path.strip_prefix("/settings/") {
        let section = section.trim_end_matches('/');
        return SETTINGS_KEYS
            .iter()
            .find(|(id, _)| *id == section)
            .map(|(_, key)| *key);
    }
    ROUTE_KEYS
        .iter()
        .find(|(p, _)| route_matches(p, path))
        .map(|(_, key)| *key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logic::glossary;

    const KEYS: [&str; 18] = [
        "login",
        "dashboard",
        "welcome",
        "experiments",
        "experiment",
        "sweep",
        "task",
        "new",
        "jobs",
        "job",
        "compare",
        "benchmarks",
        "settings-profiles",
        "settings-secrets",
        "settings-tokens",
        "settings-audit",
        "settings-system",
        "help",
    ];

    /// Every `path!("...")` in app.rs.
    fn app_routes() -> Vec<String> {
        let src = include_str!("../app.rs");
        let mut out = Vec::new();
        let mut rest = src;
        while let Some(i) = rest.find("path!(\"") {
            let tail = &rest[i + "path!(\"".len()..];
            let end = tail.find('"').unwrap();
            out.push(tail[..end].to_owned());
            rest = &tail[end..];
        }
        assert!(out.len() >= 15, "parsed too few routes: {out:?}");
        out
    }

    /// The section ids of `SECTIONS` in pages/settings/mod.rs.
    fn settings_sections() -> Vec<String> {
        let src = include_str!("../pages/settings/mod.rs");
        let start = src.find("const SECTIONS").unwrap();
        let body = &src[start..];
        let body = &body[..body.find("];").unwrap()];
        let ids: Vec<String> = body
            .lines()
            .filter_map(|l| l.trim().strip_prefix("(\""))
            .map(|l| l[..l.find('"').unwrap()].to_owned())
            .collect();
        assert!(ids.len() >= 5, "parsed too few sections: {ids:?}");
        ids
    }

    fn served(href: &str) -> bool {
        let path = href.split('#').next().unwrap();
        if let Some(section) = path.strip_prefix("/settings/") {
            return settings_sections().iter().any(|s| s == section);
        }
        app_routes().iter().any(|p| route_matches(p, path))
    }

    fn texts(p: &PageHelp) -> Vec<&'static str> {
        let mut t = vec![p.name, p.lead];
        for s in p.steps {
            t.extend([s.title, s.body]);
            t.extend(s.cmd);
            t.extend(s.link.map(|l| l.label));
        }
        for f in p.faq {
            t.extend([f.q, f.a]);
            t.extend(f.cmd);
            t.extend(f.link.map(|l| l.label));
        }
        t
    }

    fn links(p: &PageHelp) -> Vec<Link> {
        p.steps
            .iter()
            .filter_map(|s| s.link)
            .chain(p.faq.iter().filter_map(|f| f.link))
            .collect()
    }

    #[test]
    fn keys_unique_well_formed_and_complete() {
        for (i, p) in PAGES.iter().enumerate() {
            assert!(
                !p.key.is_empty() && p.key.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'),
                "{}",
                p.key
            );
            assert!(
                PAGES[i + 1..].iter().all(|o| o.key != p.key),
                "duplicate {}",
                p.key
            );
        }
        for k in KEYS {
            assert!(find(k).is_some(), "missing {k}");
        }
        assert_eq!(PAGES.len(), KEYS.len(), "unexpected extra page help");
    }

    #[test]
    fn every_page_has_enough_help() {
        for p in PAGES {
            assert!(!p.name.trim().is_empty(), "{}: name", p.key);
            assert!(!p.lead.trim().is_empty(), "{}: lead", p.key);
            assert!(p.steps.len() >= 3, "{}: {} steps", p.key, p.steps.len());
            assert!(p.faq.len() >= 4, "{}: {} questions", p.key, p.faq.len());
            for s in p.steps {
                assert!(
                    !s.title.trim().is_empty() && !s.body.trim().is_empty(),
                    "{}",
                    p.key
                );
            }
            for f in p.faq {
                assert!(
                    !f.q.trim().is_empty() && !f.a.trim().is_empty(),
                    "{}",
                    p.key
                );
            }
        }
    }

    #[test]
    fn text_is_plain_and_local() {
        for p in PAGES {
            for t in texts(p) {
                assert!(
                    !t.contains("http://") && !t.contains("https://"),
                    "{}: external URL in {t:?}",
                    p.key
                );
                assert_eq!(
                    t.matches('`').count() % 2,
                    0,
                    "{}: unbalanced backticks in {t:?}",
                    p.key
                );
            }
        }
    }

    #[test]
    fn markup_like_text_only_inside_code() {
        for p in PAGES {
            for t in texts(p) {
                for para in paragraphs(t) {
                    for r in para {
                        if let Run::Text(s) = r {
                            assert!(
                                !s.as_bytes().windows(2).any(|w| w[0] == b'<'
                                    && (w[1].is_ascii_alphabetic()
                                        || w[1] == b'/'
                                        || w[1] == b'!')),
                                "{}: tag-like text outside code in {t:?}",
                                p.key
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn links_point_at_served_routes() {
        for p in PAGES {
            for l in links(p) {
                assert!(l.href.starts_with('/'), "{}: {}", p.key, l.href);
                assert!(!l.label.trim().is_empty(), "{}: empty label", p.key);
                assert!(
                    served(l.href),
                    "{}: {} is not a served route",
                    p.key,
                    l.href
                );
                if let Some(term) = l.href.strip_prefix("/help#term-") {
                    assert!(
                        glossary::find(term).is_some(),
                        "{}: no glossary term {term}",
                        p.key
                    );
                } else if let Some(anchor) = l.href.strip_prefix("/help#") {
                    assert!(
                        ["glossary", "components", "faq", "shortcuts"].contains(&anchor),
                        "{}: unknown help anchor {anchor}",
                        p.key
                    );
                    assert!(include_str!("../pages/help.rs").contains(&format!("id=\"{anchor}\"")));
                } else {
                    assert!(
                        !l.href.contains('#'),
                        "{}: unchecked fragment {}",
                        p.key,
                        l.href
                    );
                }
            }
        }
    }

    #[test]
    fn every_route_and_section_has_help() {
        for r in app_routes() {
            if r == "/settings" || r == "/settings/:section" {
                continue;
            }
            let key = key_for_route(&r).unwrap_or_else(|| panic!("no help for {r}"));
            assert!(find(key).is_some(), "{r} -> {key} has no entry");
        }
        for s in settings_sections() {
            let key = key_for_route(&format!("/settings/{s}"))
                .unwrap_or_else(|| panic!("no help for settings section {s}"));
            assert_eq!(key, format!("settings-{s}"));
            assert!(find(key).is_some());
        }
        assert_eq!(key_for_route("/settings"), None);
        assert_eq!(key_for_route("/settings/:section"), None);
        assert_eq!(key_for_route("/settings/nope"), None);
    }

    #[test]
    fn key_for_concrete_paths() {
        assert_eq!(key_for_route("/"), Some("dashboard"));
        assert_eq!(key_for_route("/experiments/demo"), Some("experiment"));
        assert_eq!(
            key_for_route("/experiments/demo/sweeps/demo-oc-agg1"),
            Some("sweep")
        );
        assert_eq!(
            key_for_route("/experiments/demo/sweeps/demo-oc-agg1/tasks/t1"),
            Some("task")
        );
        assert_eq!(key_for_route("/jobs/0190c5f0"), Some("job"));
        assert_eq!(key_for_route("/settings/system"), Some("settings-system"));
        assert_eq!(key_for_route("/compare?a=x&b=y"), Some("compare"));
        assert_eq!(key_for_route("/nowhere/at/all"), None);
    }

    #[test]
    fn paragraphs_split_and_mark_code() {
        assert_eq!(
            paragraphs("Run `h2workbench doctor` now.\n\nThen `this`."),
            vec![
                vec![
                    Run::Text("Run "),
                    Run::Code("h2workbench doctor"),
                    Run::Text(" now.")
                ],
                vec![Run::Text("Then "), Run::Code("this"), Run::Text(".")],
            ]
        );
        assert_eq!(
            paragraphs("`a``b`"),
            vec![vec![Run::Code("a"), Run::Code("b")]]
        );
        assert_eq!(paragraphs("\n\n  \n\n"), Vec::<Vec<Run<'_>>>::new());
        assert_eq!(paragraphs("one\ntwo"), vec![vec![Run::Text("one\ntwo")]]);
    }

    #[test]
    fn unbalanced_backtick_is_text() {
        assert_eq!(
            paragraphs("a `b` c ` d"),
            vec![vec![Run::Text("a "), Run::Code("b"), Run::Text(" c ` d")]]
        );
        assert_eq!(paragraphs("<b>x</b>"), vec![vec![Run::Text("<b>x</b>")]]);
    }

    #[test]
    fn faq_filter() {
        let f = &find("jobs").unwrap().faq;
        assert_eq!(filter_faq(f, "").len(), f.len());
        assert_eq!(filter_faq(f, "   ").len(), f.len());
        let hits = filter_faq(f, "PARTIAL code");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].q.contains("partial"));
        assert!(filter_faq(f, "partial zzzz-nothing").is_empty());
    }
}
