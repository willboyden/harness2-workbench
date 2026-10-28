//! The glossary, FAQ and keyboard shortcuts shown on the Help page. Terms are linkable by
//! id (`/help#term-<id>`) and power the `Term` tooltips across the app.

pub(crate) struct Term {
    pub(crate) id: &'static str,
    pub(crate) term: &'static str,
    /// One sentence, used in tooltips.
    pub(crate) short: &'static str,
    /// The full explanation on the Help page.
    pub(crate) long: &'static str,
    pub(crate) see_also: &'static [&'static str],
}

pub(crate) const TERMS: &[Term] = &[
    Term {
        id: "harness",
        term: "Harness",
        short: "Everything around the model that makes it an agent: instructions, rules, memory, tool advice, helpers and code.",
        long: "A harness is the scaffolding that turns a language model into a working agent. harness2 never changes the model; it edits the harness, which is a small tree of files split into eight components (c1–c8). Better harnesses make the same model do better work.",
        see_also: &["components", "task-recursion", "always-on"],
    },
    Term {
        id: "components",
        term: "Components c1–c8",
        short: "The eight editable parts of a harness: system prompt, guardrails, memory, tool guidance, sub-agents, skills, scripts, plugins.",
        long: "c1 system prompt, c2 guardrails, c3 long-term memory and c4 tool guidance are always-on text the agent reads on every task. c5 sub-agents are helpers with their own context, c6 skills are workflows loaded on demand, c7 scripts are deterministic helper programs, and c8 plugins (OpenCode) or hooks (Codex) act inside the tool loop. On Codex, AGENTS.md carries c1–c4.",
        see_also: &["harness", "always-on"],
    },
    Term {
        id: "always-on",
        term: "Always-on budget",
        short: "c1–c4 are read on every task, so their combined size is capped: 24,000 characters on OpenCode and Turnstone, 8,000 on Codex, 20,000 on Hermes Agent.",
        long: "The always-on components (system prompt, guardrails, memory, tool guidance) cost context on every single task. harness2 rejects a harness whose always-on text exceeds the surface's cap, so improvers must move detail into skills, sub-agents or scripts instead. The harness viewer shows the meter.",
        see_also: &["components"],
    },
    Term {
        id: "task-recursion",
        term: "Task-level recursion",
        short: "Improving the harness on one task: propose edits, probe them on that same task, compose a refined harness.",
        long: "For each task, harness2 runs Propose–Probe–Compose: an improver model proposes harness edits, each edited harness is probed by actually running the task, and the evidence is composed into refined final harnesses. A blind selector then adopts one. Parallel mode explores in width, sequential mode in depth.",
        see_also: &["ppc", "parallel", "sequential", "k", "harnessing-recursion"],
    },
    Term {
        id: "harnessing-recursion",
        term: "Harnessing-level recursion",
        short: "Improving across tasks: each adopted harness and its lesson go into a per-domain experience library that later tasks read.",
        long: "Above the single task, harness2 accumulates experience. Every adopted harness and the lesson the reflector drew from it are stored in the domain's experience library, and later tasks of the same domain start from that library. This is why tasks within one domain run in order, while different domains can run concurrently (workers).",
        see_also: &["library", "reflection", "domain"],
    },
    Term {
        id: "ppc",
        term: "Propose–Probe–Compose",
        short: "The unit of task-level recursion: propose harness edits, probe each by running the task, compose what worked.",
        long: "Propose: an improver session reads the task, the current harness and evidence, and proposes an edit (a general INV edit or a domain-specific DOM edit). Probe: the edited harness runs the task once (a rollout), producing evidence. Compose: another improver session combines the probed edits into a final harness (AGG in parallel mode, COMPOSE in sequential mode).",
        see_also: &["task-recursion", "rollout", "improver-session"],
    },
    Term {
        id: "parallel",
        term: "Parallel mode",
        short: "Width: probe k general and k domain candidates from the same start, then make k independent compositions.",
        long: "Parallel mode (on disk: agg) explores many directions at once. From the starting harness it proposes and probes k general (inv1..invk) and k domain (dom1..domk) candidates, then composes k finals (agg1..aggk) independently. The selector picks among the finals and candidates.",
        see_also: &["sequential", "k", "selection"],
    },
    Term {
        id: "sequential",
        term: "Sequential mode",
        short: "Depth: k successive Propose–Probe–Compose steps, each starting from the previous composition.",
        long: "Sequential mode (on disk: seq) refines one line of harnesses. Step i proposes and probes edits to the current harness and composes c_i, which becomes the starting point of step i+1. The selector picks among the compositions c1..ck.",
        see_also: &["parallel", "k"],
    },
    Term {
        id: "k",
        term: "k (recursion budget)",
        short: "Per task and mode: 4k executor rollouts and 3k improver sessions.",
        long: "k sets how much search each task gets. Per task and mode harness2 spends 4k rollouts (k shared base runs, 2k candidates, k finals) and 3k improver sessions, plus a selector session when k > 1 and a reflection session per adoption. The base rollouts are shared, so running both modes costs k + 6k rollouts per task. The Workbench caps k at 32.",
        see_also: &["rollout", "improver-session", "noise-floor"],
    },
    Term {
        id: "rollout",
        term: "Rollout",
        short: "One execution of the task by the executor agent with a given harness.",
        long: "A rollout is a single run of the executor (OpenCode, Codex, Hermes Agent or Turnstone) on the task, with one harness installed. It produces a deliverable (the work product the judge scores) and a trajectory (the agent's actions and observations). The rollout browser shows both.",
        see_also: &["executor", "trajectory"],
    },
    Term {
        id: "trajectory",
        term: "Trajectory",
        short: "The record of what the agent did during a rollout: actions, observations and errors.",
        long: "Trajectories interleave [ACTION] (a tool call the agent made), [OBSERVATION] (what came back) and [ERROR] blocks. The rollout browser highlights each kind. Selectors may compare trajectories instead of deliverables.",
        see_also: &["rollout"],
    },
    Term {
        id: "executor",
        term: "Executor",
        short: "The agent framework that runs rollouts: OpenCode (default) or Codex, and for LAB Hermes Agent or Turnstone when harness2 ships them. Upstream calls it the substrate.",
        long: "OpenCode runs from a pinned source checkout through Bun and uses the OpenCode harness surface. Codex is the OpenAI Codex CLI against a Responses-API endpoint (on Vertex through the Codex bridge) and uses the Codex surface, whose always-on cap is smaller. Upstream runs Codex with its sandbox disabled. Hermes Agent and Turnstone (LAB only, in a harness2 that adds them) call the profile's OpenAI-compatible endpoint: Hermes Agent reads its harness from SOUL.md and memories/MEMORY.md in its own home, Turnstone from a persona and its instructions setting on a throwaway server per rollout. The improver stays on OpenCode for every executor.",
        see_also: &["components", "isolation"],
    },
    Term {
        id: "improver-session",
        term: "Improver session",
        short: "One call to the improver model that proposes or composes harness edits (with at most one retry).",
        long: "Improver sessions write harness edits. Each stage of the recursion is one session; if its proposal is invalid it is retried once, and if it fails again the stage falls back. The Audit tab lists every session with its model, cost, attempts and wall time, and shows the mission it received and the response it gave.",
        see_also: &["fell-back", "kept-base"],
    },
    Term {
        id: "candidate",
        term: "Candidate",
        short: "A harness from one proposal (INV general edit or DOM domain edit), probed with one rollout.",
        long: "Candidates are labelled inv1..invk (general, task-independent edits) and dom1..domk (domain-specific edits). Each is probed once; its rollout is evidence for composition, and the selector may adopt a candidate directly.",
        see_also: &["final", "base"],
    },
    Term {
        id: "final",
        term: "Final",
        short: "A composed harness: agg1..aggk in parallel mode, c1..ck in sequential mode.",
        long: "Finals combine what the probes learned. Their rollouts are what the selector usually chooses between. In parallel mode the finals are independent, so the Workbench also reports how similar they are (mean line-Jaccard).",
        see_also: &["candidate", "selection"],
    },
    Term {
        id: "base",
        term: "Base",
        short: "The unmodified starting harness; base1..basek are its reference rollouts, shared by both modes.",
        long: "Every task first runs the unmodified harness k times. Those reference rollouts anchor the comparison: judging the bases (judge --bases) gives the score to beat. Once they are judged, a sweep's Tasks table shows each task's base score and every mode's difference from it, and Compare offers the sweep's starting harness as a side, so refinement can be tested against the harness it started from, task by task.",
        see_also: &["kept-base"],
    },
    Term {
        id: "selection",
        term: "Blind selection",
        short: "A selector compares anonymised runs (A, B, …) without knowing which harness made them, and adopts one.",
        long: "After the finals run, a selector session sees the candidate runs under anonymous ids, compares their deliverables (or trajectories), and picks one. The task page shows the id map, so you can see which label each letter was, and the selector's stated reason. With k = 1 there is a single final, which is adopted without a selector session.",
        see_also: &["reflection", "judge"],
    },
    Term {
        id: "reflection",
        term: "Reflection & lesson",
        short: "After an adoption, a reflector distils what worked into a lesson: cue, lesson, evidence and confidence.",
        long: "The lesson records when it applies (cue), what to do (lesson), why the reflector believes it (evidence) and how sure it is (confidence), plus a per-edit account. Lessons are stored in the experience library so later tasks of the domain can use them.",
        see_also: &["library", "harnessing-recursion"],
    },
    Term {
        id: "library",
        term: "Experience library",
        short: "Per-domain store of adopted harnesses and lessons that later tasks of the domain start from.",
        long: "Each (mode, domain) has its own library. The Library tab of a sweep lists every adoption in task order: which harness was adopted, why, its components, and the lesson.",
        see_also: &["harnessing-recursion", "reflection", "domain"],
    },
    Term {
        id: "domain",
        term: "Domain",
        short: "A group of related tasks: an occupation (JobBench), a practice area (LAB) or a category (WorkBuddy).",
        long: "Domains share an experience library. Tasks within a domain run in order because each reads the library entries of the ones before it; different domains run concurrently up to the number of workers.",
        see_also: &["library"],
    },
    Term {
        id: "judge",
        term: "Judge",
        short: "A separate model that scores the selected run's deliverable against the task's rubric, after selection.",
        long: "Judging happens after the sweep, on the run the selector adopted (or, with --bases, on the reference runs). It never influences selection. Each graded task gets a partial-credit score (criteria passed) and an all-pass flag.",
        see_also: &["partial-credit", "all-pass", "ungraded", "cloud-judge"],
    },
    Term {
        id: "cloud-judge",
        term: "Cloud judge",
        short: "A judge model at a cloud provider (Anthropic, OpenAI, Google and others). It sees every task and output it grades, so use one only with generated or synthetic data.",
        long: "The Workbench places a profile's judge by its hosts: loopback, private addresses, single-label names (containers, services) and local-only domains are your own infrastructure; anything else, or an endpoint set through a secret, counts as cloud. An administrator can declare internal gateways private ([judge] private_hosts) or refuse cloud judges altogether ([judge] cloud = \"refuse\"). Judging and pipelines with a cloud judge ask you to confirm that the experiment's data is generated or synthetic.",
        see_also: &["judge", "profile"],
    },
    Term {
        id: "partial-credit",
        term: "Partial credit (score)",
        short: "The fraction of rubric criteria a run passed, averaged over graded tasks.",
        long: "A task with 7 of 10 criteria met scores 70%. The sweep's score is the mean over graded tasks only. Some benchmarks report points instead of criteria; the report warns when units are mixed.",
        see_also: &["all-pass", "ungraded"],
    },
    Term {
        id: "all-pass",
        term: "All-pass rate",
        short: "The share of graded tasks where every criterion passed. Stricter than partial credit.",
        long: "All-pass counts only perfect deliverables. It moves less than partial credit and is the better signal for work that must be fully correct.",
        see_also: &["partial-credit"],
    },
    Term {
        id: "ungraded",
        term: "Ungraded ≠ zero",
        short: "A task the judge could not score is excluded from the denominator, not counted as zero.",
        long: "Judge failures or tasks with no pick are ungraded. They do not drag the mean down; instead the report states how many there were. Many ungraded tasks make a score less trustworthy, so re-judge before comparing.",
        see_also: &["judge", "partial-credit"],
    },
    Term {
        id: "noise-floor",
        term: "Noise floor",
        short: "How far the same harness moves between runs on the same task: a difference within it can be run-to-run noise.",
        long: "Rollouts are random, so the same harness scores differently on the same task from run to run. Compare measures that variation when it can: between the starting harnesses of two sweeps of the same profile (the same harness, the same tasks, two runs), or between the k reference rollouts of each task (k of 2 or more). A difference within it may be noise. At k = 1 each task also adopts its single final without a choice, so scores inherit the full randomness of one rollout; repeat with other seeds or use a larger k before concluding. The report adds a NOTE when this applies.",
        see_also: &["k", "sign-test"],
    },
    Term {
        id: "fell-back",
        term: "Fell back",
        short: "A stage's proposal was rejected twice, so the stage inherited the current harness unchanged.",
        long: "When an improver's proposal is invalid (for example it breaks the always-on cap or the tree rules) it is retried once. If the retry also fails, the stage falls back: it keeps the harness it started from. Many fallbacks suggest the improver model struggles with the format.",
        see_also: &["improver-session", "kept-base"],
    },
    Term {
        id: "kept-base",
        term: "Kept base",
        short: "The accepted proposal changed nothing, or the selector preferred the unmodified harness.",
        long: "On a stage, kept base means the accepted proposal made no change. In a report, kept base counts tasks where the selector adopted the base harness over every edit — a sign that the edits did not help on those tasks.",
        see_also: &["fell-back", "selection"],
    },
    Term {
        id: "sweep",
        term: "Sweep",
        short: "One (benchmark cell, k, seed, tag) group of tasks inside an experiment.",
        long: "A sweep is identified by its summary namespace, for example lab-oc-agg1-s7. It holds both modes: summaries and judge rows live in the parallel namespace, harnesses and rollouts in each mode's own namespace.",
        see_also: &["namespace", "experiment"],
    },
    Term {
        id: "experiment",
        term: "Experiment",
        short: "One harness2 runs directory managed by the Workbench, pinned to one model profile.",
        long: "Experiments stay plain harness2 directories, so harness2's own tools keep working on them. The profile is pinned at creation because harness2 refuses to resume or judge a namespace under different models.",
        see_also: &["sweep", "profile"],
    },
    Term {
        id: "namespace",
        term: "Namespace",
        short: "<bench>-<executor>-<mode><k>[-s<seed>][-<tag>], for example lab-oc-agg1-s7.",
        long: "harness2 names its run directories by namespace. agg means parallel mode and seq sequential; s7 is seed 7. A sweep is named after its parallel (summary) namespace.",
        see_also: &["sweep"],
    },
    Term {
        id: "cell",
        term: "Cell",
        short: "A (benchmark, subset, executor) combination; every harness2 command takes one.",
        long: "Examples: LAB with OpenCode, WorkBuddy-Bench office with Codex. WorkBuddy needs a subset (office, web or code); the others must not have one.",
        see_also: &["executor"],
    },
    Term {
        id: "seed",
        term: "Seed",
        short: "Shuffles the task stream order reproducibly. It does not control model sampling.",
        long: "Without a seed, tasks run in upstream's sorted reference order. Because the experience library carries lessons forward, order matters; seeds let you repeat a sweep in another order to see how robust a result is.",
        see_also: &["noise-floor"],
    },
    Term {
        id: "split",
        term: "Train/test split",
        short: "A reproducible, domain-stratified partition of a benchmark's tasks.",
        long: "Create a split (Benchmarks page) to run the recursion on the train part and evaluate on the test part. A split has a seed and a train fraction (default 0.5).",
        see_also: &["seed"],
    },
    Term {
        id: "ledger",
        term: "Ledger",
        short: "Budget accounting per task and mode: recorded rollouts R and sessions P must equal 4k and 3k.",
        long: "The task page compares the recorded counts with the budget identity. A mismatch means something was skipped or repeated, for example after a crash and resume.",
        see_also: &["k"],
    },
    Term {
        id: "profile",
        term: "Model profile",
        short: "Which models harness2 uses (solver, improver, judge, Codex) and how to reach them.",
        long: "Profiles come in three kinds: Google Vertex AI (upstream's setup), any OpenAI-compatible endpoint, or raw environment variables. API keys are referenced by secret name, never stored in the profile.",
        see_also: &["experiment"],
    },
    Term {
        id: "isolation",
        term: "Isolation",
        short: "Where jobs run: directly on the server host (upstream behaviour) or in a hardened container.",
        long: "Host isolation gives the executor the server user's files and network, including the Workbench's own database (stored secrets, token hashes): a malicious or prompt-injected executor is effectively a Workbench administrator, so starting a run requires an explicit acknowledgement. Container isolation runs each job in a locked-down container with an egress allowlist; it is the boundary. WorkBuddy and the Codex bridge refuse container isolation in this version.",
        see_also: &["executor"],
    },
    Term {
        id: "pipeline",
        term: "Pipeline",
        short: "check → run → judge (→ judge bases) → report as one job, stopping at the first failure.",
        long: "A pipeline is the usual way to start a sweep: it checks readiness, runs the recursion, judges the selected runs (and optionally the base runs), and builds the report.",
        see_also: &["judge", "preview"],
    },
    Term {
        id: "preview",
        term: "Preview (dry run)",
        short: "harness2 --dry-run: the exact task stream, namespaces and domains, without calling any model.",
        long: "Always preview before a paid run: it shows exactly which tasks will run in which order and feeds the cost estimate.",
        see_also: &["pipeline"],
    },
    Term {
        id: "sign-test",
        term: "Sign test",
        short: "Counts tasks where B beat A and A beat B (ties dropped); p says how often such a split happens by chance.",
        long: "The sign test makes no assumption about score distributions. With n decisive tasks, it asks how likely a split at least as lopsided would be if A and B were equally good. Fewer than 6 decisive tasks can never reach p < 0.05.",
        see_also: &["noise-floor"],
    },
    Term {
        id: "spend",
        term: "Recorded spend",
        short: "harness2 records improver and selection spend only; executor rollout spend is not recorded.",
        long: "Spend figures and the budget cap use what harness2 writes into task summaries. Executor rollouts usually cost more than improver sessions and are not included, so treat recorded spend as a lower bound and check your provider's billing.",
        see_also: &["k"],
    },
    Term {
        id: "fingerprint",
        term: "Fingerprint",
        short: "A 16-hex hash of a harness tree, used to recognise identical harnesses.",
        long: "Two harnesses with the same fingerprint are byte-for-byte the same tree, whatever their labels.",
        see_also: &[],
    },
];

pub(crate) fn find(id: &str) -> Option<&'static Term> {
    TERMS.iter().find(|t| t.id == id)
}

pub(crate) struct Faq {
    pub(crate) q: &'static str,
    pub(crate) a: &'static str,
}

pub(crate) const FAQ: &[Faq] = &[
    Faq {
        q: "What does the demo do, and does it cost anything?",
        a: "harness2-demo runs the real recursion and library workflow with deterministic local editors instead of models. It needs no credentials and no network, finishes in about a second, and writes a small demo experiment you can explore like a real one.",
    },
    Faq {
        q: "Why are there no scores on my sweep?",
        a: "Scores come from judging, which runs after the sweep. Start a pipeline (which judges automatically) or a judge job for the sweep. Until then the report has no rows.",
    },
    Faq {
        q: "My job finished as “partial”. Is that bad?",
        a: "Partial means harness2 exited with code 2: the sweep finished, but some tasks failed or stayed ungraded. Look at the failed tasks on the sweep page; re-running the same command resumes and retries them.",
    },
    Faq {
        q: "Why did the spend cap stop my run later than expected?",
        a: "The cap is checked as task summaries land, and a task already in flight finishes first. harness2 also records only improver and selection spend, not executor rollouts, so real cost is higher than the recorded figure.",
    },
    Faq {
        q: "Can I change an experiment's model profile?",
        a: "No. harness2 refuses to resume or judge a namespace under different models, so an experiment keeps one profile: the one chosen when it was created or, without one, the profile of its first job that writes into it (a run, pipeline or judge job). Create a new experiment for another profile.",
    },
    Faq {
        q: "Is it safe to run jobs on the host?",
        a: "Host isolation is how upstream works: the executor agent can read and write what the server user can, and reach the network. That includes the Workbench's own database (stored secrets, token hashes), so a malicious or prompt-injected executor is effectively a Workbench administrator. Container isolation, configured by an administrator, is the boundary: use it for anything you would not run on your own machine.",
    },
    Faq {
        q: "How do I reproduce an experiment?",
        a: "Download its manifest from the experiment page. It records the harness2 commit, dependency pins, model ids, environment variable names (never values), isolation, every job's exact command and a digest of all results.",
    },
    Faq {
        q: "Where do API tokens come from?",
        a: "The first admin token is printed once by `h2workbench init`. Admins create more under Settings → Tokens; each token is shown once, so copy it straight into a password manager.",
    },
];

pub(crate) struct Shortcut {
    pub(crate) keys: &'static str,
    pub(crate) action: &'static str,
}

pub(crate) const SHORTCUTS: &[Shortcut] = &[
    Shortcut {
        keys: "Ctrl K / ⌘ K",
        action: "Open the command palette",
    },
    Shortcut {
        keys: "?",
        action: "Show keyboard shortcuts",
    },
    Shortcut {
        keys: "g then d",
        action: "Go to the dashboard",
    },
    Shortcut {
        keys: "g then e",
        action: "Go to experiments",
    },
    Shortcut {
        keys: "g then j",
        action: "Go to jobs",
    },
    Shortcut {
        keys: "g then n",
        action: "Start a new run",
    },
    Shortcut {
        keys: "g then c",
        action: "Go to compare",
    },
    Shortcut {
        keys: "g then b",
        action: "Go to benchmarks",
    },
    Shortcut {
        keys: "g then s",
        action: "Go to settings",
    },
    Shortcut {
        keys: "g then h",
        action: "Go to help",
    },
    Shortcut {
        keys: "Esc",
        action: "Close the open dialog or palette",
    },
];

/// The route a `g <key>` chord leads to.
pub(crate) fn chord_target(key: &str) -> Option<&'static str> {
    match key {
        "d" => Some("/"),
        "e" => Some("/experiments"),
        "j" => Some("/jobs"),
        "n" => Some("/new"),
        "c" => Some("/compare"),
        "b" => Some("/benchmarks"),
        "s" => Some("/settings/profiles"),
        "h" => Some("/help"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_unique_and_references_resolve() {
        for (i, t) in TERMS.iter().enumerate() {
            assert!(!t.term.is_empty() && !t.short.is_empty() && !t.long.is_empty());
            assert!(
                t.id.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'),
                "{}",
                t.id
            );
            assert!(
                TERMS[i + 1..].iter().all(|o| o.id != t.id),
                "duplicate {}",
                t.id
            );
            for s in t.see_also {
                assert!(find(s).is_some(), "{} refers to missing {s}", t.id);
                assert_ne!(*s, t.id, "{} refers to itself", t.id);
            }
        }
    }

    #[test]
    fn required_terms_present() {
        for id in [
            "harness",
            "components",
            "task-recursion",
            "harnessing-recursion",
            "ppc",
            "parallel",
            "sequential",
            "k",
            "rollout",
            "improver-session",
            "selection",
            "reflection",
            "library",
            "judge",
            "partial-credit",
            "all-pass",
            "ungraded",
            "noise-floor",
            "fell-back",
            "kept-base",
        ] {
            assert!(find(id).is_some(), "missing {id}");
        }
    }

    #[test]
    fn chords() {
        assert_eq!(chord_target("j"), Some("/jobs"));
        assert_eq!(chord_target("x"), None);
        assert!(!FAQ.is_empty() && !SHORTCUTS.is_empty());
    }
}
