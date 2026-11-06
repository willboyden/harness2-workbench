//! `h2workbench.toml` key reference: what each key is, what changing it does, accepted values
//! and default, when it takes effect. Shared by the web UI's configuration editor (its (i)
//! pop-ups and typed controls); the server's tests check it covers every key of the daemon's
//! `FileConfig` and nothing else.
//!
//! The text follows `docs/ADMIN-GUIDE.md` §5 (Configuration reference) and the validator in
//! `h2w-daemon`'s `config.rs`. It is plain text: `backticks` mark code, nothing is HTML.

/// How a key's value is written in TOML, and so which control edits it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigValueKind {
    Bool,
    Integer,
    Float,
    String,
    /// An array of strings.
    StringList,
    /// One of these TOML strings.
    Enum(&'static [&'static str]),
    /// Edited as TOML text (`egress.allow`, `benchmarks.not_offered`).
    Complex,
}

/// The reference entry of one key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigKeyHelp {
    /// Dotted path, e.g. `server.listen` or `auth.proxy.user_header`.
    pub path: &'static str,
    pub kind: ConfigValueKind,
    /// The default as a TOML literal, as `h2workbench init` writes it; empty when the default
    /// is not static (then `values` says what it is).
    pub default: &'static str,
    /// What the key is.
    pub what: &'static str,
    /// What changing it does, including its security impact where it has one.
    pub effect: &'static str,
    /// Accepted values (as the server validates them) and the default in words.
    pub values: &'static str,
    /// When a change takes effect.
    pub applies: &'static str,
    /// The key widens or narrows who can reach the server or what jobs can reach.
    pub security: bool,
}

/// The reference entry of one table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigSectionHelp {
    /// Dotted table path, e.g. `server` or `isolation.container`.
    pub path: &'static str,
    pub title: &'static str,
    pub about: &'static str,
}

const RESTART: &str = "When `h2workbench serve` starts: the server reads and validates the file once, so restart it after a change.";
const RESTART_JOBS: &str = "When `h2workbench serve` starts (restart it after a change); jobs already running keep the settings they started with.";
const RESTART_PROXY: &str = "When `h2workbench egress-proxy` starts: restart the proxy process after a change. `h2workbench serve` does not use this key.";

/// Every table of the file, in the order of `deploy/examples/h2workbench.toml`.
pub const CONFIG_SECTIONS: &[ConfigSectionHelp] = &[
    ConfigSectionHelp {
        path: "server",
        title: "HTTP listener",
        about: "Where the server listens, which `Host` names and proxies it trusts, and its request, stream, login and connection limits.",
    },
    ConfigSectionHelp {
        path: "auth",
        title: "Authentication",
        about: "How callers sign in: API tokens (the web UI exchanges one for a session cookie), or a trusted reverse proxy that asserts the user. Tokens work in both modes.",
    },
    ConfigSectionHelp {
        path: "auth.proxy",
        title: "Reverse-proxy sign-in",
        about: "Used only when `auth.mode = \"proxy\"`: the headers a trusted reverse proxy sends, the shared secret it proves itself with, and which groups get which role.",
    },
    ConfigSectionHelp {
        path: "paths",
        title: "Paths",
        about: "Where the database, job logs, experiments and the harness2 checkout live, and which harness2 commits may be installed. `~` and `~/` are expanded; other `~user` forms are refused.",
    },
    ConfigSectionHelp {
        path: "jobs",
        title: "Job engine",
        about: "How many jobs run and wait at once, how long they may run, and how much log and disk space they may use.",
    },
    ConfigSectionHelp {
        path: "isolation",
        title: "Isolation",
        about: "Where jobs run: on the host as the server's user (upstream's behaviour), or each in a hardened container.",
    },
    ConfigSectionHelp {
        path: "isolation.container",
        title: "Container isolation",
        about: "Used only when `isolation.mode = \"container\"`: the container runtime, the pinned runner image, the network and egress proxy, and the resource limits of each job container.",
    },
    ConfigSectionHelp {
        path: "egress",
        title: "Egress proxy",
        about: "Used only by `h2workbench egress-proxy`, the allowlisting proxy job containers reach the network through. Only listed hosts are contacted.",
    },
    ConfigSectionHelp {
        path: "telemetry",
        title: "Logs, traces and metrics",
        about: "The log format and level, optional OTLP trace export, and the secret that may read `/metrics`.",
    },
    ConfigSectionHelp {
        path: "benchmarks",
        title: "Benchmarks",
        about: "Benchmarks this server neither sets up nor runs, for example because another Workbench runs them.",
    },
    ConfigSectionHelp {
        path: "judge",
        title: "Cloud judges",
        about: "How judge and pipeline jobs whose profile's judge runs at a cloud provider are treated, and which public-looking judge hosts are really on your own infrastructure.",
    },
];

/// Every key of the file, in the order of `deploy/examples/h2workbench.toml`.
pub const CONFIG_KEYS: &[ConfigKeyHelp] = &[
    // ----- [server]
    ConfigKeyHelp {
        path: "server.listen",
        kind: ConfigValueKind::String,
        default: "\"127.0.0.1:8722\"",
        what: "The address and port the server binds to.",
        effect: "A loopback address keeps the server on this machine. Another address exposes it to that network; put a TLS reverse proxy in front instead of listening beyond loopback directly. Port 0 picks a free port, which the server prints once listening.",
        values: "`ip:port` with an IP, not a name (`[::1]:8722` for IPv6). Default `127.0.0.1:8722`.",
        applies: "When `h2workbench serve` starts (restart it after a change). The environment variable `H2W_LISTEN`, when set, wins over the file.",
        security: true,
    },
    ConfigKeyHelp {
        path: "server.public_url",
        kind: ConfigValueKind::String,
        default: "\"http://127.0.0.1:8722\"",
        what: "The URL people type to reach the Workbench.",
        effect: "Its host and port join the `Host` allowlist, its origin is the expected `Origin` of browser requests, and `https` makes the session cookie `Secure` and names it `__Host-h2w_session`. An `https` URL with an empty `server.trusted_proxies` draws a warning.",
        values: "An `http` or `https` URL with a host; no credentials, query, fragment or path prefix. Default `http://127.0.0.1:8722`.",
        applies: "When `h2workbench serve` starts (restart it after a change). The environment variable `H2W_PUBLIC_URL`, when set, wins over the file.",
        security: true,
    },
    ConfigKeyHelp {
        path: "server.allowed_hosts",
        kind: ConfigValueKind::StringList,
        default: "[]",
        what: "Extra `Host` header values the server answers, such as a LAN name or the name a reverse proxy forwards.",
        effect: "Requests with any other `Host` get `421`, the defence against DNS rebinding. Each entry widens which names reach the server; an entry without a port matches any port.",
        values: "Each `host`, `host:port`, `[v6]` or `[v6]:port` with a valid host name or IP. Default empty: the `public_url` host and the loopback forms of the bound port only.",
        applies: RESTART,
        security: true,
    },
    ConfigKeyHelp {
        path: "server.trusted_proxies",
        kind: ConfigValueKind::StringList,
        default: "[]",
        what: "Reverse proxies whose `X-Forwarded-For` and `x-request-id` headers are believed and, in proxy sign-in mode, whose identity headers are accepted.",
        effect: "The client address becomes the right-most forwarded address that is not itself trusted, so rate limits and the audit log see real clients. Trusted peers are exempt from `server.max_connections_per_ip`. Trust only your own proxy: a trusted peer can claim any client address, and in proxy mode any user.",
        values: "Each an IP address or CIDR, such as `192.0.2.10` or `198.51.100.0/24`. Default empty: no peer is trusted.",
        applies: RESTART,
        security: true,
    },
    ConfigKeyHelp {
        path: "server.max_body_bytes",
        kind: ConfigValueKind::Integer,
        default: "1048576",
        what: "The largest request body the server accepts.",
        effect: "Larger bodies are refused with `413`. Raising it lets larger profiles, harness validations and similar requests through, at the cost of more memory per request.",
        values: "1024 to 1073741824 bytes. Default 1048576 (1 MiB).",
        applies: RESTART,
        security: false,
    },
    ConfigKeyHelp {
        path: "server.request_timeout_secs",
        kind: ConfigValueKind::Integer,
        default: "60",
        what: "The longest a request may take.",
        effect: "A request that takes longer is answered `503`.",
        values: "1 to 3600 seconds. Default 60.",
        applies: RESTART,
        security: false,
    },
    ConfigKeyHelp {
        path: "server.sse_max_connections",
        kind: ConfigValueKind::Integer,
        default: "64",
        what: "How many live job event streams may be open at once, in total.",
        effect: "Further streams are refused with `503` until one closes.",
        values: "1 to 10000. Default 64.",
        applies: RESTART,
        security: false,
    },
    ConfigKeyHelp {
        path: "server.sse_max_per_principal",
        kind: ConfigValueKind::Integer,
        default: "8",
        what: "How many event streams one principal may hold: a token together with all its browser sessions, or one proxy user.",
        effect: "Further streams from that principal are refused with `429` and `Retry-After: 5`, so one caller cannot use up `server.sse_max_connections`.",
        values: "1 to 10000. Default 8.",
        applies: RESTART,
        security: false,
    },
    ConfigKeyHelp {
        path: "server.max_connections_per_ip",
        kind: ConfigValueKind::Integer,
        default: "256",
        what: "How many open connections one client address (IPv6: one /64) may hold, out of the server's 1024 in total.",
        effect: "Further connections are closed when accepted and counted in `h2w_connections_refused_total{reason=\"per_client\"}`. Peers in `server.trusted_proxies` are exempt.",
        values: "1 to 1024. Default 256.",
        applies: RESTART,
        security: false,
    },
    ConfigKeyHelp {
        path: "server.login_attempts_per_minute",
        kind: ConfigValueKind::Integer,
        default: "10",
        what: "Sign-in attempts, successful or not, allowed per client address (IPv6 per /64) and minute.",
        effect: "A token bucket of this size refilled over a minute; when it is empty, sign-in answers `429` with `Retry-After`. Lower values slow down token guessing; behind a reverse proxy, list it in `server.trusted_proxies` or every client shares one bucket.",
        values: "1 to 10000. Default 10.",
        applies: RESTART,
        security: false,
    },
    // ----- [auth]
    ConfigKeyHelp {
        path: "auth.mode",
        kind: ConfigValueKind::Enum(&["token", "proxy"]),
        default: "\"token\"",
        what: "How callers authenticate.",
        effect: "`token`: API tokens (the web UI exchanges one for a session cookie). `proxy`: a trusted reverse proxy asserts the user and groups in headers, and tokens keep working. `proxy` needs a non-empty `server.trusted_proxies` and the shared secret in the environment variable named by `auth.proxy.secret_env` when the server starts.",
        values: "`token` or `proxy`. Default `token`.",
        applies: RESTART,
        security: true,
    },
    ConfigKeyHelp {
        path: "auth.session_ttl_hours",
        kind: ConfigValueKind::Integer,
        default: "12",
        what: "The lifetime of a browser session.",
        effect: "Shorter sessions mean signing in more often; longer ones keep a stolen session cookie useful for longer. A session never outlives the token it was made from.",
        values: "1 to 8760 hours. Default 12.",
        applies: "When `h2workbench serve` starts (restart it after a change); it applies to sessions created after the restart.",
        security: true,
    },
    // ----- [auth.proxy]
    ConfigKeyHelp {
        path: "auth.proxy.user_header",
        kind: ConfigValueKind::String,
        default: "\"x-forwarded-user\"",
        what: "The header in which the reverse proxy sends the authenticated user name.",
        effect: "Used only in `auth.mode = \"proxy\"`, and only from peers in `server.trusted_proxies` that also present the shared secret. It must match what your proxy sends.",
        values: "A valid HTTP header name. Default `x-forwarded-user`.",
        applies: RESTART,
        security: true,
    },
    ConfigKeyHelp {
        path: "auth.proxy.groups_header",
        kind: ConfigValueKind::String,
        default: "\"x-forwarded-groups\"",
        what: "The header in which the reverse proxy sends the user's groups, comma-separated (several header lines are merged).",
        effect: "The groups decide the role through `admin_groups`, `operator_groups` and `viewer_groups`. Used only in proxy mode.",
        values: "A valid HTTP header name. Default `x-forwarded-groups`.",
        applies: RESTART,
        security: true,
    },
    ConfigKeyHelp {
        path: "auth.proxy.secret_header",
        kind: ConfigValueKind::String,
        default: "\"x-h2w-proxy-secret\"",
        what: "The header in which the reverse proxy presents the shared secret.",
        effect: "Identity headers are accepted only with the right secret in this header, so a client that reaches the server around the proxy cannot claim a user. Used only in proxy mode.",
        values: "A valid HTTP header name. Default `x-h2w-proxy-secret`.",
        applies: RESTART,
        security: true,
    },
    ConfigKeyHelp {
        path: "auth.proxy.secret_env",
        kind: ConfigValueKind::String,
        default: "\"H2W_PROXY_SECRET\"",
        what: "The NAME of the environment variable that holds the shared proxy secret. The secret itself never goes in this file.",
        effect: "In proxy mode the server reads the secret from this variable when it starts; its value must be at least 16 characters.",
        values: "An environment variable name: `[A-Z_][A-Z0-9_]*`, at most 128 characters. Default `H2W_PROXY_SECRET`.",
        applies: RESTART,
        security: true,
    },
    ConfigKeyHelp {
        path: "auth.proxy.admin_groups",
        kind: ConfigValueKind::StringList,
        default: "[]",
        what: "Proxy groups whose members become `admin`.",
        effect: "Admins manage profiles, secrets, tokens and the audit log, install harness2 and set up benchmarks. Keep this list short.",
        values: "Group names as your proxy sends them. Default empty.",
        applies: RESTART,
        security: true,
    },
    ConfigKeyHelp {
        path: "auth.proxy.operator_groups",
        kind: ConfigValueKind::StringList,
        default: "[]",
        what: "Proxy groups whose members become `operator`, unless they are admins.",
        effect: "Operators also create experiments and start and cancel jobs.",
        values: "Group names as your proxy sends them. Default empty.",
        applies: RESTART,
        security: true,
    },
    ConfigKeyHelp {
        path: "auth.proxy.viewer_groups",
        kind: ConfigValueKind::StringList,
        default: "[]",
        what: "Proxy groups whose members become `viewer`.",
        effect: "Empty: every authenticated proxy user is at least a viewer. Non-empty: users in none of the three group lists are refused with `403`.",
        values: "Group names as your proxy sends them. Default empty (every proxy user is a viewer).",
        applies: RESTART,
        security: true,
    },
    // ----- [paths]
    ConfigKeyHelp {
        path: "paths.data_dir",
        kind: ConfigValueKind::String,
        default: "\"\"",
        what: "The directory for the database `workbench.db`, `logs/jobs/`, `jobs/` (per-job temporary files) and `splits/`.",
        effect: "Pointing it elsewhere starts from that directory's database: profiles, tokens, jobs and the audit log live there. It is created with mode 0700.",
        values: "An absolute path or `~/...`. Empty (the default) = `$XDG_DATA_HOME/harness2-workbench`, normally `~/.local/share/harness2-workbench`.",
        applies: "When `h2workbench serve` starts (restart it after a change). The environment variable `H2W_DATA_DIR`, when set, wins over the file.",
        security: false,
    },
    ConfigKeyHelp {
        path: "paths.experiments_dir",
        kind: ConfigValueKind::String,
        default: "\"\"",
        what: "The directory holding the experiments: each experiment is one directory here, one `HARNESS2_RUNS_DIR`.",
        effect: "Moving it makes the server look for experiments in the new place; existing experiment directories are not moved for you.",
        values: "An absolute path or `~/...`. Empty (the default) = `<data_dir>/experiments`.",
        applies: RESTART,
        security: false,
    },
    ConfigKeyHelp {
        path: "paths.harness2_home",
        kind: ConfigValueKind::String,
        default: "\"\"",
        what: "The harness2 checkout, with its `.venv` and `dependencies/`.",
        effect: "Jobs run harness2 from here. With `paths.harness2_managed = true` the Workbench installs and updates it in this directory.",
        values: "An absolute path or `~/...`. Empty (the default) = `<data_dir>/harness2`.",
        applies: RESTART,
        security: false,
    },
    ConfigKeyHelp {
        path: "paths.harness2_managed",
        kind: ConfigValueKind::Bool,
        default: "true",
        what: "Whether the Workbench installs and updates harness2 in `paths.harness2_home`.",
        effect: "`true`: `install_harness2` jobs install and update the checkout. `false`: an existing checkout you manage yourself, which the Workbench never modifies; `install_harness2` is refused.",
        values: "`true` or `false`. Default `true`.",
        applies: RESTART,
        security: false,
    },
    ConfigKeyHelp {
        path: "paths.allow_untested_harness2_rev",
        kind: ConfigValueKind::Bool,
        default: "false",
        what: "Whether `install_harness2` may check out a commit other than the one this release was tested against.",
        effect: "`false`: another `rev` is refused with `422`. `true` accepts any 40-hex commit. Installing runs the checkout's build code, and a fork can make any commit fetchable from the upstream repository, so turn it on only when you have reviewed the commit you install.",
        values: "`true` or `false`. Default `false`.",
        applies: RESTART,
        security: true,
    },
    // ----- [jobs]
    ConfigKeyHelp {
        path: "jobs.max_concurrent",
        kind: ConfigValueKind::Integer,
        default: "2",
        what: "How many jobs run at once (the Codex bridge is outside the count).",
        effect: "Further jobs wait in the queue. Higher values finish more work in parallel and use more CPU, memory and model capacity.",
        values: "1 to 64. Default 2.",
        applies: RESTART,
        security: false,
    },
    ConfigKeyHelp {
        path: "jobs.max_queued",
        kind: ConfigValueKind::Integer,
        default: "100",
        what: "How many jobs may wait in the queue.",
        effect: "A new job beyond it is refused with `503`.",
        values: "1 to 100000. Default 100.",
        applies: RESTART,
        security: false,
    },
    ConfigKeyHelp {
        path: "jobs.timeout_hours",
        kind: ConfigValueKind::Float,
        default: "48.0",
        what: "The wall-clock limit of one job.",
        effect: "A job that runs longer ends `failed` with `timed out after ...`.",
        values: "Greater than 0 and at most 8760 hours; fractions are allowed (`0.5` = 30 minutes). Default 48.",
        applies: RESTART_JOBS,
        security: false,
    },
    ConfigKeyHelp {
        path: "jobs.budget_poll_secs",
        kind: ConfigValueKind::Integer,
        default: "30",
        what: "How often the budget guard reads the recorded spend while a `run` step runs.",
        effect: "Shorter intervals stop an over-budget run sooner and read the results more often.",
        values: "1 to 3600 seconds. Default 30.",
        applies: RESTART_JOBS,
        security: false,
    },
    ConfigKeyHelp {
        path: "jobs.log_retention_days",
        kind: ConfigValueKind::Integer,
        default: "90",
        what: "How long job log files are kept.",
        effect: "Log files older than this are deleted; the job rows stay.",
        values: "1 to 36500 days. Default 90.",
        applies: RESTART,
        security: false,
    },
    ConfigKeyHelp {
        path: "jobs.max_log_bytes",
        kind: ConfigValueKind::Integer,
        default: "268435456",
        what: "The largest log stored per job.",
        effect: "Past it the log gets one marker line and further output is read (the job never blocks) and parsed for progress, but not stored. The server's own lines may use 64 KiB more.",
        values: "65536 to 1099511627776 bytes. Default 268435456 (256 MiB).",
        applies: RESTART_JOBS,
        security: false,
    },
    ConfigKeyHelp {
        path: "jobs.min_free_disk_mb",
        kind: ConfigValueKind::Integer,
        default: "256",
        what: "The free space the data directory's filesystem must keep while jobs run.",
        effect: "A running job is stopped, `failed` with `stopped: less than N MiB free on the data directory`, when free space drops below it; checked when a job starts and every 15 seconds. `0` = never.",
        values: "0 to 1099511627776 MiB. Default 256.",
        applies: RESTART_JOBS,
        security: false,
    },
    // ----- [isolation]
    ConfigKeyHelp {
        path: "isolation.mode",
        kind: ConfigValueKind::Enum(&["host", "container"]),
        default: "\"host\"",
        what: "Where jobs run.",
        effect: "`host`: jobs run as the server's user, with its files and network (upstream's behaviour), and `run` and `pipeline` jobs must acknowledge the risks. `container`: each job runs in a hardened container configured under `[isolation.container]`; WorkBuddy-Bench needs host isolation, so its jobs are refused on such a server.",
        values: "`host` or `container`. Default `host`.",
        applies: RESTART_JOBS,
        security: true,
    },
    ConfigKeyHelp {
        path: "isolation.preacknowledge_host_risks",
        kind: ConfigValueKind::Bool,
        default: "false",
        what: "Whether an administrator accepts the risks of host isolation for everyone.",
        effect: "`false`: `run` and `pipeline` jobs in host mode must carry `acknowledge_risks: true` or are refused with `422`. `true` accepts every such run without the flag, including runs started by MCP agents, and the run wizard shows a note instead of asking. Set it only where every operator has been told and no agent holds an operator token.",
        values: "`true` or `false`. Default `false`.",
        applies: RESTART,
        security: true,
    },
    // ----- [isolation.container]
    ConfigKeyHelp {
        path: "isolation.container.runtime",
        kind: ConfigValueKind::Enum(&["docker", "podman"]),
        default: "\"docker\"",
        what: "The container CLI that starts job containers.",
        effect: "Used only in `isolation.mode = \"container\"`. The CLI must be installed for the server's user; the doctor checks that it answers.",
        values: "`docker` or `podman`. Default `docker`.",
        applies: RESTART_JOBS,
        security: false,
    },
    ConfigKeyHelp {
        path: "isolation.container.image",
        kind: ConfigValueKind::String,
        default: "",
        what: "The runner image every job container starts from.",
        effect: "A pinned reference makes every job run the same, reviewed image. Build or pull it before starting jobs.",
        values: "`name:tag` with a tag other than `latest`, or `name@sha256:<64 hex>`; the repository name in lowercase. Default `harness2-workbench-runner:<this Workbench version>`.",
        applies: RESTART_JOBS,
        security: true,
    },
    ConfigKeyHelp {
        path: "isolation.container.network",
        kind: ConfigValueKind::String,
        default: "\"none\"",
        what: "The container network job containers join.",
        effect: "`none`: no network at all (the demo works; model calls and installs do not). A network name puts jobs on it, normally an internal network whose only way out is an `h2workbench egress-proxy`. `host`, `bridge` and `default` are refused because they would give jobs unrestricted network access.",
        values: "`none`, or a user-defined network name (`[A-Za-z0-9][A-Za-z0-9_.-]*`, at most 64 characters). Default `none`.",
        applies: RESTART_JOBS,
        security: true,
    },
    ConfigKeyHelp {
        path: "isolation.container.proxy",
        kind: ConfigValueKind::String,
        default: "\"\"",
        what: "The egress proxy as job containers reach it, such as `http://h2w-egress:3128`.",
        effect: "Set inside each job as `HTTPS_PROXY`, `HTTP_PROXY` (and lowercase), with `NO_PROXY=localhost,127.0.0.1,::1`. It appears on the runtime's command line, so it never carries credentials.",
        values: "Empty, or `http://host:port` with an explicit port and no credentials, path or query; needs a `network` other than `none`. Default empty: no proxy variables.",
        applies: RESTART_JOBS,
        security: true,
    },
    ConfigKeyHelp {
        path: "isolation.container.cpus",
        kind: ConfigValueKind::Float,
        default: "4.0",
        what: "CPUs each job container may use (`--cpus`).",
        effect: "Lower values protect the host from one busy job; higher values speed up jobs that build or run code.",
        values: "Greater than 0 and at most 1024. Default 4.0.",
        applies: RESTART_JOBS,
        security: false,
    },
    ConfigKeyHelp {
        path: "isolation.container.memory_mb",
        kind: ConfigValueKind::Integer,
        default: "8192",
        what: "Memory each job container may use (`--memory`).",
        effect: "A job that needs more is killed by the runtime.",
        values: "128 to 4194304 MiB. Default 8192.",
        applies: RESTART_JOBS,
        security: false,
    },
    ConfigKeyHelp {
        path: "isolation.container.pids_limit",
        kind: ConfigValueKind::Integer,
        default: "2048",
        what: "Processes and threads each job container may have (`--pids-limit`).",
        effect: "Caps runaway process creation inside a job.",
        values: "16 to 1000000. Default 2048.",
        applies: RESTART_JOBS,
        security: false,
    },
    ConfigKeyHelp {
        path: "isolation.container.user",
        kind: ConfigValueKind::String,
        default: "\"\"",
        what: "The user job containers run as.",
        effect: "It decides which files the job can write in its mounts. Empty detects it: `0:0` on a rootless runtime (container root is your own unprivileged user there), the server's own `uid:gid` on a rootful one.",
        values: "Empty, or a numeric `uid` or `uid:gid`. Default empty (detected).",
        applies: RESTART_JOBS,
        security: true,
    },
    // ----- [egress]
    ConfigKeyHelp {
        path: "egress.listen",
        kind: ConfigValueKind::String,
        default: "\"127.0.0.1:3128\"",
        what: "Where `h2workbench egress-proxy` binds.",
        effect: "The proxy has no client access list: anyone who can reach this address can use the allowlist. Loopback by default; a deployment listens on its internal jobs network (for example `0.0.0.0:3128` inside the proxy's own container) so job containers can reach it.",
        values: "`ip:port`. Default `127.0.0.1:3128`.",
        applies: RESTART_PROXY,
        security: true,
    },
    ConfigKeyHelp {
        path: "egress.allow",
        kind: ConfigValueKind::Complex,
        default: "[]",
        what: "The only hosts the egress proxy contacts.",
        effect: "Empty refuses every request (the proxy warns at start-up). Each entry lets jobs reach that host, so list only the model, judge and install hosts the jobs need. Loopback, private, link-local (cloud metadata), CGNAT and unique-local addresses are refused unless the matching entry sets `allow_private`.",
        values: "An array whose entries are `\"host\"`, `\"*.suffix\"` (the domain's subdomains), or a table `{ host = \"...\", allow_private = true, ports = [4000] }`: `host` a host name (IP literals are refused), `allow_private` lets that host resolve to private addresses (default false), `ports` the ports it may be reached on (default 443 and 80; 0 is refused). Default empty.",
        applies: RESTART_PROXY,
        security: true,
    },
    // ----- [telemetry]
    ConfigKeyHelp {
        path: "telemetry.log_format",
        kind: ConfigValueKind::Enum(&["pretty", "json"]),
        default: "\"pretty\"",
        what: "The format of the server's log lines on standard error.",
        effect: "`pretty`: human-readable. `json`: one JSON object per line, for log collectors.",
        values: "`pretty` or `json`. Default `pretty`.",
        applies: "When `h2workbench serve` starts (restart it after a change). The environment variable `H2W_LOG_FORMAT`, when set, wins over the file.",
        security: false,
    },
    ConfigKeyHelp {
        path: "telemetry.log_level",
        kind: ConfigValueKind::String,
        default: "\"info\"",
        what: "Log verbosity.",
        effect: "More verbose levels log more detail about requests and jobs; logs never contain secrets, tokens or request bodies. The server does not read `RUST_LOG`; this key replaces it.",
        values: "A level or filter directive, such as `info`, `debug` or `h2w_daemon=debug,warn`. Default `info`.",
        applies: RESTART,
        security: false,
    },
    ConfigKeyHelp {
        path: "telemetry.otlp_endpoint",
        kind: ConfigValueKind::String,
        default: "\"\"",
        what: "The OTLP/HTTP collector traces are exported to, such as `http://127.0.0.1:4318`.",
        effect: "Empty: no trace export. A URL without a path gets `/v1/traces` appended. Traces leave the server for that collector, so point it at one you run.",
        values: "Empty, or an `http` or `https` URL with a host. Default empty.",
        applies: "When `h2workbench serve` starts (restart it after a change). The environment variable `H2W_OTLP_ENDPOINT`, when set, wins over the file.",
        security: false,
    },
    ConfigKeyHelp {
        path: "telemetry.service_name",
        kind: ConfigValueKind::String,
        default: "\"harness2-workbench\"",
        what: "The `service.name` of exported traces.",
        effect: "Tells several Workbenches apart in one trace backend.",
        values: "1 to 128 characters. Default `harness2-workbench`.",
        applies: RESTART,
        security: false,
    },
    ConfigKeyHelp {
        path: "telemetry.metrics_token_secret",
        kind: ConfigValueKind::String,
        default: "\"\"",
        what: "The NAME of a secret whose value is accepted as a bearer token on `/metrics`, for Prometheus. The value itself is set under Settings, Secrets, never in this file.",
        effect: "Empty: `/metrics` needs a viewer credential. Set: whoever holds that secret's value can read the metrics without a token.",
        values: "Empty, or a secret name `[A-Z][A-Z0-9_]{0,63}`. Default empty.",
        applies: RESTART,
        security: true,
    },
    // ----- [benchmarks]
    ConfigKeyHelp {
        path: "benchmarks.not_offered",
        kind: ConfigValueKind::Complex,
        default: "[]",
        what: "Benchmarks this server neither sets up nor runs, for example because another Workbench runs them.",
        effect: "Their cards say not on this server with the note and a link, the New run wizard will not start them, and every job for them (setup and splits included) is refused with `409` and the note. An entry also replaces the needs host isolation notice WorkBuddy-Bench gets under container isolation.",
        values: "`[[benchmarks.not_offered]]` tables, each with `bench` (`jb`, `lab` or `wb`; each at most once), an optional `note` (at most 300 characters, no control characters) and an optional `url` (an `http` or `https` URL with a host and no credentials, at most 2048 characters). Default none.",
        applies: RESTART,
        security: false,
    },
    // ----- [judge]
    ConfigKeyHelp {
        path: "judge.cloud",
        kind: ConfigValueKind::Enum(&["acknowledge", "refuse"]),
        default: "\"acknowledge\"",
        what: "How judge and pipeline jobs whose profile's judge is not local are treated. A cloud judge sees every task and output it grades.",
        effect: "`acknowledge`: such jobs are refused with `422` until the caller sets `acknowledge_cloud_judge`, confirming the data is generated or synthetic. `refuse`: they are refused with `409` whatever the caller says. Run jobs never call the judge and are not affected.",
        values: "`acknowledge` or `refuse`. Default `acknowledge`.",
        applies: RESTART,
        security: true,
    },
    ConfigKeyHelp {
        path: "judge.private_hosts",
        kind: ConfigValueKind::StringList,
        default: "[]",
        what: "Judge hosts on your own infrastructure that a public-looking name would otherwise mark as cloud, such as `llm.corp.example.com`.",
        effect: "Judges on these hosts count as local, so their jobs need no confirmation. The classification looks at host names only: a gateway listed here that forwards to a cloud provider still sends the data there.",
        values: "Host names, IP addresses or `.domain` suffixes (`.corp.example.com` covers the domain and its subdomains); no scheme, port, path or wildcard; at most 253 characters each. Default empty.",
        applies: RESTART,
        security: true,
    },
];

/// The entry of the key at `path` (dotted).
#[must_use]
pub fn config_key(path: &str) -> Option<&'static ConfigKeyHelp> {
    CONFIG_KEYS.iter().find(|k| k.path == path)
}

/// The entry of the table at `path` (dotted).
#[must_use]
pub fn config_section(path: &str) -> Option<&'static ConfigSectionHelp> {
    CONFIG_SECTIONS.iter().find(|s| s.path == path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section_of(path: &str) -> &str {
        path.rsplit_once('.').map_or("", |(s, _)| s)
    }

    #[test]
    fn paths_are_unique() {
        for (i, k) in CONFIG_KEYS.iter().enumerate() {
            assert!(
                CONFIG_KEYS[i + 1..].iter().all(|o| o.path != k.path),
                "duplicate key {}",
                k.path
            );
        }
        for (i, s) in CONFIG_SECTIONS.iter().enumerate() {
            assert!(
                CONFIG_SECTIONS[i + 1..].iter().all(|o| o.path != s.path),
                "duplicate section {}",
                s.path
            );
        }
    }

    #[test]
    fn keys_belong_to_known_sections_and_sections_have_keys() {
        for k in CONFIG_KEYS {
            assert!(
                config_section(section_of(k.path)).is_some(),
                "{} has no section entry",
                k.path
            );
        }
        for s in CONFIG_SECTIONS {
            assert!(
                CONFIG_KEYS.iter().any(|k| section_of(k.path) == s.path),
                "section {} has no keys",
                s.path
            );
            assert!(!s.title.is_empty() && !s.about.is_empty(), "{}", s.path);
        }
    }

    #[test]
    fn every_entry_is_filled_in() {
        for k in CONFIG_KEYS {
            for (name, text) in [
                ("what", k.what),
                ("effect", k.effect),
                ("values", k.values),
                ("applies", k.applies),
            ] {
                assert!(!text.trim().is_empty(), "{}: empty {name}", k.path);
                assert!(
                    !text.contains("</") && !text.contains("<a ") && !text.contains("]("),
                    "{}: {name} looks like markup or a Markdown link",
                    k.path
                );
            }
            if let ConfigValueKind::Enum(opts) = k.kind {
                assert!(!opts.is_empty(), "{}: no options", k.path);
            }
        }
    }

    #[test]
    fn lookups() {
        assert_eq!(
            config_key("server.listen").map(|k| k.default),
            Some("\"127.0.0.1:8722\"")
        );
        assert!(config_key("server.nope").is_none());
        assert!(config_section("isolation.container").is_some());
        assert!(config_section("nope").is_none());
    }
}
