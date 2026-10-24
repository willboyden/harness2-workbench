//! Configuration: `h2studio.toml` (ARCHITECTURE.md §11), environment overrides and
//! validation.
//!
//! Location: `--config`, else `$H2S_CONFIG`, else
//! `$XDG_CONFIG_HOME/harness2-studio/h2studio.toml`. Unknown keys are errors. Environment
//! overrides: `H2S_LISTEN`, `H2S_DATA_DIR`, `H2S_PUBLIC_URL`, `H2S_LOG_FORMAT`,
//! `H2S_OTLP_ENDPOINT`. Every value is validated once, at load time, so the server either
//! starts with a coherent configuration or refuses with every problem listed.
//!
//! Environment access goes through an injected lookup function so tests never mutate the
//! process environment.

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use h2s_api::{AuthMode, Isolation};
use ipnet::IpNet;
use serde::{Deserialize, Serialize};

/// Application directory name under the XDG config and data directories.
pub const APP_DIR: &str = "harness2-studio";
/// Configuration file name.
pub const CONFIG_FILE: &str = "h2studio.toml";
/// Environment variable naming the configuration file.
pub const ENV_CONFIG: &str = "H2S_CONFIG";
/// Prefix of secrets supplied through the environment (`H2S_SECRET_<NAME>`).
pub const ENV_SECRET_PREFIX: &str = "H2S_SECRET_";

/// A function that reads one environment variable.
pub type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<String>;

/// The process environment (the production [`EnvLookup`]).
#[must_use]
pub fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

// ---------------------------------------------------------------------------------------
// File format
// ---------------------------------------------------------------------------------------

/// `h2studio.toml` as written by people.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct FileConfig {
    pub server: ServerSection,
    pub auth: AuthSection,
    pub paths: PathsSection,
    pub jobs: JobsSection,
    pub isolation: IsolationSection,
    pub egress: EgressSection,
    pub telemetry: TelemetrySection,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ServerSection {
    pub listen: String,
    pub public_url: String,
    pub allowed_hosts: Vec<String>,
    pub trusted_proxies: Vec<String>,
    pub max_body_bytes: u64,
    pub request_timeout_secs: u64,
    pub sse_max_connections: u32,
    pub login_attempts_per_minute: u32,
}

impl Default for ServerSection {
    fn default() -> Self {
        ServerSection {
            listen: "127.0.0.1:8722".to_owned(),
            public_url: "http://127.0.0.1:8722".to_owned(),
            allowed_hosts: Vec::new(),
            trusted_proxies: Vec::new(),
            max_body_bytes: 1_048_576,
            request_timeout_secs: 60,
            sse_max_connections: 64,
            login_attempts_per_minute: 10,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AuthSection {
    pub mode: AuthMode,
    pub session_ttl_hours: u32,
    pub proxy: ProxySection,
}

impl Default for AuthSection {
    fn default() -> Self {
        AuthSection {
            mode: AuthMode::Token,
            session_ttl_hours: 12,
            proxy: ProxySection::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ProxySection {
    pub user_header: String,
    pub groups_header: String,
    pub secret_header: String,
    pub secret_env: String,
    pub admin_groups: Vec<String>,
    pub operator_groups: Vec<String>,
    /// Empty = any authenticated proxy user is a viewer.
    pub viewer_groups: Vec<String>,
}

impl Default for ProxySection {
    fn default() -> Self {
        ProxySection {
            user_header: "x-forwarded-user".to_owned(),
            groups_header: "x-forwarded-groups".to_owned(),
            secret_header: "x-h2s-proxy-secret".to_owned(),
            secret_env: "H2S_PROXY_SECRET".to_owned(),
            admin_groups: Vec::new(),
            operator_groups: Vec::new(),
            viewer_groups: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PathsSection {
    /// Empty = `$XDG_DATA_HOME/harness2-studio` (normally `~/.local/share/harness2-studio`).
    pub data_dir: String,
    /// Empty = `<data_dir>/experiments`.
    pub experiments_dir: String,
    /// Empty = `<data_dir>/harness2`.
    pub harness2_home: String,
    /// `false` = point at an existing checkout, which is never modified.
    pub harness2_managed: bool,
}

impl Default for PathsSection {
    fn default() -> Self {
        PathsSection {
            data_dir: String::new(),
            experiments_dir: String::new(),
            harness2_home: String::new(),
            harness2_managed: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct JobsSection {
    pub max_concurrent: u32,
    pub max_queued: u32,
    /// Wall-clock limit per job; fractions allowed.
    pub timeout_hours: f64,
    pub budget_poll_secs: u64,
    pub log_retention_days: u32,
}

impl Default for JobsSection {
    fn default() -> Self {
        JobsSection {
            max_concurrent: 2,
            max_queued: 100,
            timeout_hours: 48.0,
            budget_poll_secs: 30,
            log_retention_days: 90,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct IsolationSection {
    pub mode: Isolation,
    pub preacknowledge_host_risks: bool,
    pub container: ContainerSection,
}

impl Default for IsolationSection {
    fn default() -> Self {
        IsolationSection {
            mode: Isolation::Host,
            preacknowledge_host_risks: false,
            container: ContainerSection::default(),
        }
    }
}

/// The container runtime CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeKind {
    Docker,
    Podman,
}

impl RuntimeKind {
    #[must_use]
    pub fn core(self) -> h2s_core::isolation::Runtime {
        match self {
            RuntimeKind::Docker => h2s_core::isolation::Runtime::Docker,
            RuntimeKind::Podman => h2s_core::isolation::Runtime::Podman,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            RuntimeKind::Docker => "docker",
            RuntimeKind::Podman => "podman",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ContainerSection {
    pub runtime: RuntimeKind,
    pub image: String,
    pub network: String,
    pub cpus: f64,
    pub memory_mb: u64,
    pub pids_limit: u32,
    /// Empty = the server's own host user (detected; see `jobs::container`).
    pub user: String,
    /// `http://host:port` of an `h2studio egress-proxy` reachable on `network`. Jobs get it as
    /// `HTTP(S)_PROXY`; empty = no proxy variables inside the container.
    pub proxy: String,
}

impl Default for ContainerSection {
    fn default() -> Self {
        ContainerSection {
            runtime: RuntimeKind::Docker,
            image: "harness2-studio-runner:0.1.0".to_owned(),
            network: "none".to_owned(),
            cpus: 4.0,
            memory_mb: 8192,
            pids_limit: 2048,
            user: String::new(),
            proxy: String::new(),
        }
    }
}

/// One egress allowlist entry: `"host"`, `"*.suffix"` or `{host = "...", allow_private = true}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AllowEntry {
    Host(String),
    Detailed(AllowEntryDetailed),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllowEntryDetailed {
    pub host: String,
    #[serde(default)]
    pub allow_private: bool,
}

impl AllowEntry {
    #[must_use]
    pub fn host(&self) -> &str {
        match self {
            AllowEntry::Host(h) => h,
            AllowEntry::Detailed(d) => &d.host,
        }
    }

    #[must_use]
    pub fn allow_private(&self) -> bool {
        match self {
            AllowEntry::Host(_) => false,
            AllowEntry::Detailed(d) => d.allow_private,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct EgressSection {
    pub listen: String,
    pub allow: Vec<AllowEntry>,
}

impl Default for EgressSection {
    fn default() -> Self {
        EgressSection {
            listen: "0.0.0.0:3128".to_owned(),
            allow: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogFormat {
    Pretty,
    Json,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TelemetrySection {
    pub log_format: LogFormat,
    pub log_level: String,
    /// OTLP/HTTP base URL (e.g. `http://127.0.0.1:4318`); empty = no trace export.
    pub otlp_endpoint: String,
    pub service_name: String,
    /// Secret NAME whose value is accepted as a bearer token on `/metrics`.
    pub metrics_token_secret: String,
}

impl Default for TelemetrySection {
    fn default() -> Self {
        TelemetrySection {
            log_format: LogFormat::Pretty,
            log_level: "info".to_owned(),
            otlp_endpoint: String::new(),
            service_name: "harness2-studio".to_owned(),
            metrics_token_secret: String::new(),
        }
    }
}

// ---------------------------------------------------------------------------------------
// Validated configuration
// ---------------------------------------------------------------------------------------

/// A `Host` value the server answers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostEntry {
    /// Lowercase host name or IP literal (IPv6 without brackets).
    pub host: String,
    /// `None` = any port (including none).
    pub port: Option<u16>,
}

impl HostEntry {
    /// Parse `host`, `host:port`, `[v6]` or `[v6]:port`.
    #[must_use]
    pub fn parse(s: &str) -> Option<HostEntry> {
        let (host, port) = split_host_port(s)?;
        Some(HostEntry {
            host: host.to_ascii_lowercase(),
            port,
        })
    }

    /// `true` when the parsed `Host` header `(host, port)` matches this entry.
    #[must_use]
    pub fn matches(&self, host: &str, port: Option<u16>) -> bool {
        self.host == host && (self.port.is_none() || self.port == port)
    }
}

/// Split a `Host`-style authority into host and optional port. IPv6 literals must be
/// bracketed. Returns `None` for empty hosts, bad ports or stray characters.
#[must_use]
pub fn split_host_port(s: &str) -> Option<(String, Option<u16>)> {
    if s.is_empty() || s.len() > 300 {
        return None;
    }
    let (host, port) = if let Some(rest) = s.strip_prefix('[') {
        let (h, after) = rest.split_once(']')?;
        h.parse::<std::net::Ipv6Addr>().ok()?;
        let port = match after {
            "" => None,
            p => Some(p.strip_prefix(':')?.parse::<u16>().ok()?),
        };
        (h.to_owned(), port)
    } else {
        match s.rsplit_once(':') {
            Some((h, p)) => {
                if h.contains(':') {
                    return None; // unbracketed IPv6
                }
                (h.to_owned(), Some(p.parse::<u16>().ok()?))
            }
            None => (s.to_owned(), None),
        }
    };
    let ok = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_'));
    ok.then_some((host, port))
}

/// Resolved filesystem locations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub data_dir: PathBuf,
    pub experiments_dir: PathBuf,
    pub harness2_home: PathBuf,
    pub harness2_managed: bool,
    /// `<data_dir>/studio.db`
    pub db_path: PathBuf,
    /// `<data_dir>/logs/jobs`
    pub job_logs_dir: PathBuf,
    /// `<data_dir>/jobs` (per-job generated files, deleted when the job ends)
    pub job_files_dir: PathBuf,
    /// `<data_dir>/splits`
    pub splits_dir: PathBuf,
}

impl Paths {
    #[must_use]
    pub fn from_dirs(
        data_dir: PathBuf,
        experiments_dir: PathBuf,
        harness2_home: PathBuf,
        managed: bool,
    ) -> Paths {
        Paths {
            db_path: data_dir.join("studio.db"),
            job_logs_dir: data_dir.join("logs").join("jobs"),
            job_files_dir: data_dir.join("jobs"),
            splits_dir: data_dir.join("splits"),
            data_dir,
            experiments_dir,
            harness2_home,
            harness2_managed: managed,
        }
    }

    /// Create every directory the server writes (0700).
    ///
    /// # Errors
    /// I/O errors, with the failing path in the message.
    pub fn ensure(&self) -> std::io::Result<()> {
        for dir in [
            &self.data_dir,
            &self.experiments_dir,
            &self.job_logs_dir,
            &self.job_files_dir,
            &self.splits_dir,
        ] {
            crate::util::ensure_private_dir(dir)
                .map_err(|e| std::io::Error::new(e.kind(), format!("{}: {e}", dir.display())))?;
        }
        if let Some(parent) = self.db_path.parent() {
            crate::util::ensure_private_dir(parent)?;
        }
        Ok(())
    }
}

/// An egress allowlist rule after validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowRule {
    /// Lowercase host, or the suffix after `*.` when `wildcard`.
    pub host: String,
    pub wildcard: bool,
    pub allow_private: bool,
}

/// The validated configuration the server runs with.
#[derive(Debug, Clone)]
pub struct Config {
    /// The file contents after environment overrides.
    pub file: FileConfig,
    /// The file it was loaded from (`None` = built-in defaults).
    pub source: Option<PathBuf>,
    pub listen: SocketAddr,
    pub public_url: url::Url,
    /// `scheme://host[:port]`, as browsers send it in `Origin`.
    pub public_origin: String,
    pub public_https: bool,
    /// Hosts from `public_url` and `server.allowed_hosts` (loopback forms of the bound port
    /// are added when the listener is bound).
    pub allowed_hosts: Vec<HostEntry>,
    pub trusted_proxies: Vec<IpNet>,
    pub paths: Paths,
    pub request_timeout: Duration,
    pub session_ttl: Duration,
    pub job_timeout: Duration,
    pub budget_poll: Duration,
    pub egress_listen: SocketAddr,
    pub egress_allow: Vec<AllowRule>,
    /// Full OTLP/HTTP traces URL (`.../v1/traces`), when export is configured.
    pub otlp_traces_url: Option<String>,
}

/// Why a configuration could not be used.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {message}")]
    Parse { path: String, message: String },
    #[error("invalid configuration ({path}):\n{}", .problems.iter().map(|p| format!("  - {p}")).collect::<Vec<_>>().join("\n"))]
    Invalid { path: String, problems: Vec<String> },
    #[error("{0}")]
    Other(String),
}

/// Where the configuration file is (whether or not it exists), and whether the location
/// was given explicitly (`--config` or `$H2S_CONFIG`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigLocation {
    pub path: PathBuf,
    pub explicit: bool,
}

/// Resolve the configuration file location.
///
/// # Errors
/// When no explicit path is given and no home directory can be determined.
pub fn locate(explicit: Option<&Path>, env: EnvLookup<'_>) -> Result<ConfigLocation, ConfigError> {
    if let Some(p) = explicit {
        return Ok(ConfigLocation {
            path: expand_tilde(&p.to_string_lossy(), env)?,
            explicit: true,
        });
    }
    if let Some(p) = env(ENV_CONFIG).filter(|s| !s.is_empty()) {
        return Ok(ConfigLocation {
            path: expand_tilde(&p, env)?,
            explicit: true,
        });
    }
    let base = xdg_config_home(env)?;
    Ok(ConfigLocation {
        path: base.join(APP_DIR).join(CONFIG_FILE),
        explicit: false,
    })
}

fn home_dir(env: EnvLookup<'_>) -> Result<PathBuf, ConfigError> {
    if let Some(h) = env("HOME").filter(|h| h.starts_with('/')) {
        return Ok(PathBuf::from(h));
    }
    directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .ok_or_else(|| {
            ConfigError::Other(
                "cannot determine the home directory; set HOME or give absolute paths".into(),
            )
        })
}

/// `$XDG_CONFIG_HOME` when absolute, else `~/.config` (the XDG rule `directories` follows).
fn xdg_config_home(env: EnvLookup<'_>) -> Result<PathBuf, ConfigError> {
    match env("XDG_CONFIG_HOME").filter(|v| v.starts_with('/')) {
        Some(v) => Ok(PathBuf::from(v)),
        None => Ok(home_dir(env)?.join(".config")),
    }
}

/// `$XDG_DATA_HOME` when absolute, else `~/.local/share`.
fn xdg_data_home(env: EnvLookup<'_>) -> Result<PathBuf, ConfigError> {
    match env("XDG_DATA_HOME").filter(|v| v.starts_with('/')) {
        Some(v) => Ok(PathBuf::from(v)),
        None => Ok(home_dir(env)?.join(".local").join("share")),
    }
}

/// The default data directory (`$XDG_DATA_HOME/harness2-studio`).
///
/// # Errors
/// When no home directory can be determined.
pub fn default_data_dir(env: EnvLookup<'_>) -> Result<PathBuf, ConfigError> {
    Ok(xdg_data_home(env)?.join(APP_DIR))
}

/// Expand a leading `~` / `~/` to the home directory. Other `~user` forms are refused.
///
/// # Errors
/// When `~user` is used or no home directory is known.
pub fn expand_tilde(s: &str, env: EnvLookup<'_>) -> Result<PathBuf, ConfigError> {
    if s == "~" {
        return home_dir(env);
    }
    if let Some(rest) = s.strip_prefix("~/") {
        return Ok(home_dir(env)?.join(rest));
    }
    if s.starts_with('~') {
        return Err(ConfigError::Other(format!(
            "{s}: only `~` and `~/...` are expanded"
        )));
    }
    Ok(PathBuf::from(s))
}

impl Config {
    /// Locate, read, override from the environment and validate.
    ///
    /// A missing file at the default location means built-in defaults; a missing file at an
    /// explicit location is an error.
    ///
    /// # Errors
    /// Read, parse or validation errors, each naming the file.
    pub fn load(explicit: Option<&Path>, env: EnvLookup<'_>) -> Result<Config, ConfigError> {
        let loc = locate(explicit, env)?;
        let (file, source) = match std::fs::read_to_string(&loc.path) {
            Ok(text) => (
                Self::parse_str(&text, &loc.path.display().to_string())?,
                Some(loc.path.clone()),
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !loc.explicit => {
                (FileConfig::default(), None)
            }
            Err(e) => {
                return Err(ConfigError::Read {
                    path: loc.path,
                    source: e,
                });
            }
        };
        Self::from_file(file, source, env)
    }

    /// Parse TOML text (unknown keys are errors).
    ///
    /// # Errors
    /// A parse error with the location toml reports.
    pub fn parse_str(text: &str, origin: &str) -> Result<FileConfig, ConfigError> {
        toml::from_str::<FileConfig>(text).map_err(|e| ConfigError::Parse {
            path: origin.to_owned(),
            message: e.to_string(),
        })
    }

    /// Apply environment overrides to `file` and validate it.
    ///
    /// # Errors
    /// Every validation problem at once.
    pub fn from_file(
        mut file: FileConfig,
        source: Option<PathBuf>,
        env: EnvLookup<'_>,
    ) -> Result<Config, ConfigError> {
        let mut problems = apply_env_overrides(&mut file, env);
        let origin = source.as_ref().map_or_else(
            || "built-in defaults".to_owned(),
            |p| p.display().to_string(),
        );
        let cfg = validate(file, source, env, &mut problems);
        match cfg {
            Some(cfg) if problems.is_empty() => Ok(cfg),
            _ => Err(ConfigError::Invalid {
                path: origin,
                problems,
            }),
        }
    }

    /// Default configuration with every path under `data_dir` (used by `init` and tests).
    ///
    /// # Errors
    /// Validation errors (none for an absolute `data_dir`).
    pub fn defaults_in(data_dir: &Path) -> Result<Config, ConfigError> {
        let mut file = FileConfig::default();
        file.paths.data_dir = data_dir.display().to_string();
        Self::from_file(file, None, &|_| None)
    }

    /// The configured isolation mode.
    #[must_use]
    pub fn isolation(&self) -> Isolation {
        self.file.isolation.mode
    }

    /// True when the peer address is one of `server.trusted_proxies`.
    #[must_use]
    pub fn is_trusted_proxy(&self, ip: IpAddr) -> bool {
        let ip = canonical_ip(ip);
        self.trusted_proxies.iter().any(|n| n.contains(&ip))
    }

    /// Lines describing the resolved locations, for the startup log and `config check`.
    #[must_use]
    pub fn describe_paths(&self) -> Vec<(String, String)> {
        vec![
            (
                "config file".into(),
                self.source.as_ref().map_or_else(
                    || "(none; built-in defaults)".to_owned(),
                    |p| p.display().to_string(),
                ),
            ),
            ("data dir".into(), self.paths.data_dir.display().to_string()),
            ("database".into(), self.paths.db_path.display().to_string()),
            (
                "experiments".into(),
                self.paths.experiments_dir.display().to_string(),
            ),
            (
                "harness2 home".into(),
                format!(
                    "{} ({})",
                    self.paths.harness2_home.display(),
                    if self.paths.harness2_managed {
                        "managed"
                    } else {
                        "existing checkout, never modified"
                    }
                ),
            ),
        ]
    }
}

/// IPv4-mapped IPv6 addresses compare as IPv4.
#[must_use]
pub fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        IpAddr::V4(_) => ip,
    }
}

/// Apply `H2S_*` overrides; returns problems with override values.
fn apply_env_overrides(file: &mut FileConfig, env: EnvLookup<'_>) -> Vec<String> {
    let mut problems = Vec::new();
    let get = |name: &str| env(name).filter(|v| !v.is_empty());
    if let Some(v) = get("H2S_LISTEN") {
        file.server.listen = v;
    }
    if let Some(v) = get("H2S_DATA_DIR") {
        file.paths.data_dir = v;
    }
    if let Some(v) = get("H2S_PUBLIC_URL") {
        file.server.public_url = v;
    }
    if let Some(v) = get("H2S_LOG_FORMAT") {
        match v.as_str() {
            "json" => file.telemetry.log_format = LogFormat::Json,
            "pretty" => file.telemetry.log_format = LogFormat::Pretty,
            _ => problems.push(format!("H2S_LOG_FORMAT={v}: must be pretty or json")),
        }
    }
    if let Some(v) = get("H2S_OTLP_ENDPOINT") {
        file.telemetry.otlp_endpoint = v;
    }
    problems
}

fn range_check<T: PartialOrd + std::fmt::Display + Copy>(
    problems: &mut Vec<String>,
    key: &str,
    v: T,
    lo: T,
    hi: T,
) {
    if v < lo || v > hi {
        problems.push(format!("{key}: {v} is outside {lo}..={hi}"));
    }
}

fn header_name_ok(s: &str) -> bool {
    !s.is_empty() && http::HeaderName::from_bytes(s.as_bytes()).is_ok()
}

fn env_name_ok(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_uppercase() || c == '_')
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && s.len() <= 128
}

fn resolve_dir(
    raw: &str,
    default: impl FnOnce() -> Result<PathBuf, ConfigError>,
    key: &str,
    env: EnvLookup<'_>,
    problems: &mut Vec<String>,
) -> Option<PathBuf> {
    let p = if raw.is_empty() {
        default()
    } else {
        expand_tilde(raw, env)
    };
    match p {
        Ok(p) if p.is_absolute() => Some(p),
        Ok(p) => {
            problems.push(format!(
                "{key}: {} must be absolute (or start with ~/)",
                p.display()
            ));
            None
        }
        Err(e) => {
            problems.push(format!("{key}: {e}"));
            None
        }
    }
}

/// `true` for a container image reference pinned to a tag (not `latest`) or a digest.
#[must_use]
pub fn image_is_pinned(image: &str) -> bool {
    if image.is_empty()
        || image.len() > 255
        || image.starts_with('-')
        || !image
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | ':' | '@'))
    {
        return false;
    }
    if let Some((name, digest)) = image.split_once('@') {
        let hex = digest.strip_prefix("sha256:").unwrap_or("");
        return !name.is_empty() && hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit());
    }
    // The name (everything before the tag) is lowercase, as docker's reference grammar and
    // h2s-core's container argv require; only the tag may contain capitals.
    let (prefix, last) = image.rsplit_once('/').map_or(("", image), |(p, l)| (p, l));
    let repo = last.split_once(':').map_or(last, |(r, _)| r);
    if prefix
        .chars()
        .chain(repo.chars())
        .any(|c| c.is_ascii_uppercase())
    {
        return false;
    }
    // The tag is after the last ':' that follows the last '/' (a registry port is not a tag).
    let last_segment = image.rsplit('/').next().unwrap_or(image);
    match last_segment.split_once(':') {
        Some((repo, tag)) => {
            !repo.is_empty()
                && !tag.is_empty()
                && tag != "latest"
                && tag.len() <= 128
                && !tag.contains(':')
        }
        None => false,
    }
}

/// Docker network names: `none`, or a user-defined network (never `host` or `container:`).
#[must_use]
pub fn network_ok(n: &str) -> bool {
    if n == "none" {
        return true;
    }
    let mut chars = n.chars();
    n.len() <= 64
        && n != "host"
        && n != "bridge"
        && n != "default"
        && matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// A container proxy URL: `http://host:port` with an explicit port, and no credentials, path
/// or query (the value is on the runtime's command line).
#[must_use]
pub fn proxy_url_ok(u: &str) -> bool {
    let Some(rest) = u.strip_prefix("http://") else {
        return false;
    };
    let authority = rest.strip_suffix('/').unwrap_or(rest);
    !authority.contains(['@', '/', '?', '#'])
        && split_host_port(authority).is_some_and(|(_, port)| port.is_some_and(|p| p != 0))
}

/// `uid` or `uid:gid` (numeric).
#[must_use]
pub fn container_user_ok(u: &str) -> bool {
    // h2s-core's container argv accepts numeric ids only (no name resolution in the image).
    let numeric = |s: &str| !s.is_empty() && s.len() <= 10 && s.chars().all(|c| c.is_ascii_digit());
    match u.split_once(':') {
        Some((a, b)) => numeric(a) && numeric(b),
        None => numeric(u),
    }
}

fn check_hostname(problems: &mut Vec<String>, key: &str, host: &str) {
    if host.parse::<IpAddr>().is_ok() {
        return;
    }
    if let Err(e) = h2s_core::names::hostname(host) {
        problems.push(format!("{key}: {host:?}: {e}"));
    }
}

fn validate(
    file: FileConfig,
    source: Option<PathBuf>,
    env: EnvLookup<'_>,
    problems: &mut Vec<String>,
) -> Option<Config> {
    let s = &file.server;
    let listen = match s.listen.parse::<SocketAddr>() {
        Ok(a) => Some(a),
        Err(_) => {
            problems.push(format!(
                "server.listen: {:?} is not an address like 127.0.0.1:8722",
                s.listen
            ));
            None
        }
    };

    // public_url: http(s), a host, no credentials, query, fragment or path.
    let public_url = match url::Url::parse(&s.public_url) {
        Ok(u) => {
            let mut bad = Vec::new();
            if !matches!(u.scheme(), "http" | "https") {
                bad.push("scheme must be http or https");
            }
            if u.host_str().is_none() {
                bad.push("a host is required");
            }
            if !u.username().is_empty() || u.password().is_some() {
                bad.push("credentials are not allowed");
            }
            if u.query().is_some() || u.fragment().is_some() {
                bad.push("query and fragment are not allowed");
            }
            if u.path() != "/" && !u.path().is_empty() {
                bad.push("serving under a path prefix is not supported");
            }
            if bad.is_empty() {
                Some(u)
            } else {
                problems.push(format!("server.public_url: {}", bad.join("; ")));
                None
            }
        }
        Err(e) => {
            problems.push(format!("server.public_url: {e}"));
            None
        }
    };

    let mut allowed_hosts = Vec::new();
    let mut public_origin = String::new();
    let mut public_https = false;
    if let Some(u) = &public_url {
        public_https = u.scheme() == "https";
        let host = u.host_str().unwrap_or_default().to_ascii_lowercase();
        let host = host
            .strip_prefix('[')
            .and_then(|h| h.strip_suffix(']'))
            .map_or(host.clone(), str::to_owned);
        let default_port = if public_https { 443 } else { 80 };
        let port = u.port().unwrap_or(default_port);
        allowed_hosts.push(HostEntry {
            host: host.clone(),
            port: Some(port),
        });
        if port == default_port {
            // Browsers omit the default port from `Host`.
            allowed_hosts.push(HostEntry { host, port: None });
        }
        public_origin = u.origin().ascii_serialization();
    }
    for (i, h) in s.allowed_hosts.iter().enumerate() {
        match HostEntry::parse(h) {
            Some(e) => {
                check_hostname(problems, &format!("server.allowed_hosts[{i}]"), &e.host);
                allowed_hosts.push(e);
            }
            None => problems.push(format!(
                "server.allowed_hosts[{i}]: {h:?} is not host, host:port or [v6]:port"
            )),
        }
    }

    let mut trusted_proxies = Vec::new();
    for (i, p) in s.trusted_proxies.iter().enumerate() {
        let parsed = p
            .parse::<IpNet>()
            .ok()
            .or_else(|| p.parse::<IpAddr>().ok().map(IpNet::from));
        match parsed {
            Some(n) => trusted_proxies.push(n),
            None => problems.push(format!(
                "server.trusted_proxies[{i}]: {p:?} is not an IP address or CIDR"
            )),
        }
    }

    range_check(
        problems,
        "server.max_body_bytes",
        s.max_body_bytes,
        1024,
        1 << 30,
    );
    range_check(
        problems,
        "server.request_timeout_secs",
        s.request_timeout_secs,
        1,
        3600,
    );
    range_check(
        problems,
        "server.sse_max_connections",
        s.sse_max_connections,
        1,
        10_000,
    );
    range_check(
        problems,
        "server.login_attempts_per_minute",
        s.login_attempts_per_minute,
        1,
        10_000,
    );

    let a = &file.auth;
    range_check(
        problems,
        "auth.session_ttl_hours",
        a.session_ttl_hours,
        1,
        8760,
    );
    let p = &a.proxy;
    for (key, v) in [
        ("auth.proxy.user_header", &p.user_header),
        ("auth.proxy.groups_header", &p.groups_header),
        ("auth.proxy.secret_header", &p.secret_header),
    ] {
        if !header_name_ok(v) {
            problems.push(format!("{key}: {v:?} is not a valid header name"));
        }
    }
    if !env_name_ok(&p.secret_env) {
        problems.push(format!(
            "auth.proxy.secret_env: {:?} is not an environment variable name",
            p.secret_env
        ));
    }
    if a.mode == AuthMode::Proxy && trusted_proxies.is_empty() {
        problems.push(
            "auth.mode = \"proxy\" needs server.trusted_proxies (the reverse proxy's address)"
                .to_owned(),
        );
    }

    let ps = &file.paths;
    let data_dir = resolve_dir(
        &ps.data_dir,
        || default_data_dir(env),
        "paths.data_dir",
        env,
        problems,
    );
    let experiments_dir = data_dir.as_ref().and_then(|d| {
        resolve_dir(
            &ps.experiments_dir,
            || Ok(d.join("experiments")),
            "paths.experiments_dir",
            env,
            problems,
        )
    });
    let harness2_home = data_dir.as_ref().and_then(|d| {
        resolve_dir(
            &ps.harness2_home,
            || Ok(d.join("harness2")),
            "paths.harness2_home",
            env,
            problems,
        )
    });

    let j = &file.jobs;
    range_check(problems, "jobs.max_concurrent", j.max_concurrent, 1, 64);
    range_check(problems, "jobs.max_queued", j.max_queued, 1, 100_000);
    if !(j.timeout_hours.is_finite() && j.timeout_hours > 0.0 && j.timeout_hours <= 8760.0) {
        problems.push(format!(
            "jobs.timeout_hours: {} must be > 0 and <= 8760",
            j.timeout_hours
        ));
    }
    range_check(
        problems,
        "jobs.budget_poll_secs",
        j.budget_poll_secs,
        1,
        3600,
    );
    range_check(
        problems,
        "jobs.log_retention_days",
        j.log_retention_days,
        1,
        36_500,
    );

    let c = &file.isolation.container;
    if !image_is_pinned(&c.image) {
        problems.push(format!(
            "isolation.container.image: {:?} must be pinned as name:tag (not latest) or name@sha256:<digest>",
            c.image
        ));
    }
    if !network_ok(&c.network) {
        problems.push(format!(
            "isolation.container.network: {:?} must be \"none\" or a user-defined network (never host)",
            c.network
        ));
    }
    if !(c.cpus.is_finite() && c.cpus > 0.0 && c.cpus <= 1024.0) {
        problems.push(format!(
            "isolation.container.cpus: {} must be > 0 and <= 1024",
            c.cpus
        ));
    }
    range_check(
        problems,
        "isolation.container.memory_mb",
        c.memory_mb,
        128,
        4 << 20,
    );
    range_check(
        problems,
        "isolation.container.pids_limit",
        c.pids_limit,
        16,
        1_000_000,
    );
    if !c.user.is_empty() && !container_user_ok(&c.user) {
        problems.push(format!(
            "isolation.container.user: {:?} must be a numeric uid or uid:gid",
            c.user
        ));
    }
    if !c.proxy.is_empty() {
        if !proxy_url_ok(&c.proxy) {
            problems.push(
                "isolation.container.proxy: must be http://host:port, without credentials or a path"
                    .to_owned(),
            );
        } else if c.network == "none" {
            problems.push(
                "isolation.container.proxy: needs a network; with network = \"none\" no proxy is reachable"
                    .to_owned(),
            );
        }
    }

    let e = &file.egress;
    let egress_listen = match e.listen.parse::<SocketAddr>() {
        Ok(a) => Some(a),
        Err(_) => {
            problems.push(format!("egress.listen: {:?} is not an address", e.listen));
            None
        }
    };
    let mut egress_allow = Vec::new();
    for (i, entry) in e.allow.iter().enumerate() {
        let raw = entry.host().trim_end_matches('.').to_ascii_lowercase();
        let (wildcard, host) = match raw.strip_prefix("*.") {
            Some(rest) => (true, rest.to_owned()),
            None => (false, raw.clone()),
        };
        if host.is_empty() || host.contains('*') || host.parse::<IpAddr>().is_ok() {
            problems.push(format!(
                "egress.allow[{i}]: {:?} must be a host name or *.suffix (IP literals are not allowed)",
                entry.host()
            ));
            continue;
        }
        check_hostname(problems, &format!("egress.allow[{i}]"), &host);
        egress_allow.push(AllowRule {
            host,
            wildcard,
            allow_private: entry.allow_private(),
        });
    }

    let t = &file.telemetry;
    if tracing_subscriber::EnvFilter::try_new(&t.log_level).is_err() {
        problems.push(format!(
            "telemetry.log_level: {:?} is not a level or filter directive",
            t.log_level
        ));
    }
    let otlp_traces_url = if t.otlp_endpoint.is_empty() {
        None
    } else {
        match url::Url::parse(&t.otlp_endpoint) {
            Ok(u) if matches!(u.scheme(), "http" | "https") && u.host_str().is_some() => {
                let base = t.otlp_endpoint.trim_end_matches('/');
                Some(if u.path().is_empty() || u.path() == "/" {
                    format!("{base}/v1/traces")
                } else {
                    t.otlp_endpoint.clone()
                })
            }
            _ => {
                problems.push(format!(
                    "telemetry.otlp_endpoint: {:?} must be an http(s) URL",
                    t.otlp_endpoint
                ));
                None
            }
        }
    };
    if t.service_name.is_empty() || t.service_name.len() > 128 {
        problems.push("telemetry.service_name: 1..=128 characters".to_owned());
    }
    if !t.metrics_token_secret.is_empty()
        && let Err(e) = h2s_core::names::secret_name(&t.metrics_token_secret)
    {
        problems.push(format!("telemetry.metrics_token_secret: {e}"));
    }

    let (listen, public_url, egress_listen, data_dir, experiments_dir, harness2_home) = (
        listen?,
        public_url?,
        egress_listen?,
        data_dir?,
        experiments_dir?,
        harness2_home?,
    );
    let managed = file.paths.harness2_managed;
    let hours = if j.timeout_hours.is_finite() && j.timeout_hours > 0.0 {
        j.timeout_hours.min(8760.0)
    } else {
        48.0
    };
    Some(Config {
        source,
        listen,
        public_url,
        public_origin,
        public_https,
        allowed_hosts,
        trusted_proxies,
        paths: Paths::from_dirs(data_dir, experiments_dir, harness2_home, managed),
        request_timeout: Duration::from_secs(s.request_timeout_secs.clamp(1, 3600)),
        session_ttl: Duration::from_secs(u64::from(a.session_ttl_hours.clamp(1, 8760)) * 3600),
        job_timeout: Duration::from_secs_f64(hours * 3600.0),
        budget_poll: Duration::from_secs(j.budget_poll_secs.clamp(1, 3600)),
        egress_listen,
        egress_allow,
        otlp_traces_url,
        file,
    })
}

/// The configuration file `h2studio init` writes: every section with its defaults and the
/// resolved data directory.
///
/// # Errors
/// Serialization errors (not expected).
pub fn render_initial(data_dir: &Path) -> Result<String, ConfigError> {
    let mut file = FileConfig::default();
    file.paths.data_dir = data_dir.display().to_string();
    let body = toml::to_string(&file).map_err(|e| ConfigError::Other(e.to_string()))?;
    Ok(format!(
        "# Harness² Studio configuration (docs/ARCHITECTURE.md §11).\n\
         # Unknown keys are errors. Environment overrides: H2S_LISTEN, H2S_DATA_DIR,\n\
         # H2S_PUBLIC_URL, H2S_LOG_FORMAT, H2S_OTLP_ENDPOINT.\n\n{body}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn env_none(_: &str) -> Option<String> {
        None
    }

    fn cfg_from(text: &str) -> Result<Config, ConfigError> {
        let mut file = Config::parse_str(text, "test")?;
        if file.paths.data_dir.is_empty() {
            file.paths.data_dir = "/srv/h2s".into();
        }
        Config::from_file(file, None, &env_none)
    }

    #[test]
    fn defaults_validate() {
        let c = cfg_from("").unwrap();
        assert_eq!(c.listen.to_string(), "127.0.0.1:8722");
        assert_eq!(c.public_origin, "http://127.0.0.1:8722");
        assert_eq!(
            c.paths.experiments_dir,
            PathBuf::from("/srv/h2s/experiments")
        );
        assert_eq!(c.paths.harness2_home, PathBuf::from("/srv/h2s/harness2"));
        assert_eq!(c.paths.db_path, PathBuf::from("/srv/h2s/studio.db"));
        assert_eq!(c.job_timeout, Duration::from_secs(48 * 3600));
        assert!(c.otlp_traces_url.is_none());
    }

    #[test]
    fn unknown_keys_are_errors() {
        let e = cfg_from("[server]\nlisten = \"127.0.0.1:1\"\nlistne = 1\n").unwrap_err();
        assert!(e.to_string().contains("listne"), "{e}");
        let e = cfg_from("[nope]\n").unwrap_err();
        assert!(e.to_string().contains("nope"), "{e}");
    }

    #[test]
    fn documented_example_parses() {
        let text = r#"
[server]
listen = "127.0.0.1:8722"
public_url = "http://127.0.0.1:8722"
allowed_hosts = []
trusted_proxies = []
max_body_bytes = 1048576
request_timeout_secs = 60
sse_max_connections = 64
login_attempts_per_minute = 10

[auth]
mode = "token"
session_ttl_hours = 12
[auth.proxy]
user_header = "x-forwarded-user"
groups_header = "x-forwarded-groups"
secret_header = "x-h2s-proxy-secret"
secret_env = "H2S_PROXY_SECRET"
admin_groups = []
operator_groups = []
viewer_groups = []

[paths]
data_dir = "/srv/h2s"
experiments_dir = ""
harness2_home = ""
harness2_managed = true

[jobs]
max_concurrent = 2
max_queued = 100
timeout_hours = 48
budget_poll_secs = 30
log_retention_days = 90

[isolation]
mode = "host"
preacknowledge_host_risks = false
[isolation.container]
runtime = "docker"
image = "harness2-studio-runner:0.1.0"
network = "none"
cpus = 4.0
memory_mb = 8192
pids_limit = 2048
user = ""

[egress]
listen = "0.0.0.0:3128"
allow = []

[telemetry]
log_format = "pretty"
log_level = "info"
otlp_endpoint = ""
service_name = "harness2-studio"
metrics_token_secret = ""
"#;
        let c = cfg_from(text).unwrap();
        assert_eq!(c.file.jobs.max_concurrent, 2);
    }

    #[test]
    fn env_overrides_apply() {
        let env = |n: &str| match n {
            "H2S_LISTEN" => Some("127.0.0.1:0".to_owned()),
            "H2S_DATA_DIR" => Some("/tmp/h2s-data".to_owned()),
            "H2S_PUBLIC_URL" => Some("https://studio.example.com".to_owned()),
            "H2S_LOG_FORMAT" => Some("json".to_owned()),
            "H2S_OTLP_ENDPOINT" => Some("http://127.0.0.1:4318".to_owned()),
            _ => None,
        };
        let c = Config::from_file(FileConfig::default(), None, &env).unwrap();
        assert_eq!(c.listen.port(), 0);
        assert_eq!(c.paths.data_dir, PathBuf::from("/tmp/h2s-data"));
        assert!(c.public_https);
        assert_eq!(c.public_origin, "https://studio.example.com");
        assert!(
            c.allowed_hosts
                .iter()
                .any(|h| h.matches("studio.example.com", None))
        );
        assert_eq!(c.file.telemetry.log_format, LogFormat::Json);
        assert_eq!(
            c.otlp_traces_url.as_deref(),
            Some("http://127.0.0.1:4318/v1/traces")
        );
        let bad = |n: &str| (n == "H2S_LOG_FORMAT").then(|| "xml".to_owned());
        let mut f = FileConfig::default();
        f.paths.data_dir = "/srv".into();
        let e = Config::from_file(f, None, &bad).unwrap_err();
        assert!(e.to_string().contains("H2S_LOG_FORMAT=xml"), "{e}");
    }

    #[test]
    fn tilde_and_xdg_resolution() {
        let env = |n: &str| match n {
            "HOME" => Some("/var/lib/tester".to_owned()),
            "XDG_DATA_HOME" => Some("relative/ignored".to_owned()),
            _ => None,
        };
        assert_eq!(
            expand_tilde("~/x", &env).unwrap(),
            PathBuf::from("/var/lib/tester/x")
        );
        assert!(expand_tilde("~other/x", &env).is_err());
        assert_eq!(
            default_data_dir(&env).unwrap(),
            PathBuf::from("/var/lib/tester/.local/share/harness2-studio")
        );
        let loc = locate(None, &env).unwrap();
        assert_eq!(
            loc.path,
            PathBuf::from("/var/lib/tester/.config/harness2-studio/h2studio.toml")
        );
        assert!(!loc.explicit);
        let env2 = |n: &str| match n {
            "HOME" => Some("/var/lib/tester".to_owned()),
            "H2S_CONFIG" => Some("~/c.toml".to_owned()),
            _ => None,
        };
        let loc = locate(None, &env2).unwrap();
        assert_eq!(loc.path, PathBuf::from("/var/lib/tester/c.toml"));
        assert!(loc.explicit);
    }

    #[test]
    fn invalid_values_are_all_reported() {
        let text = r#"
[server]
listen = "nope"
public_url = "ftp://x/y?z"
trusted_proxies = ["10.0.0.0/8", "garbage"]
max_body_bytes = 1
[jobs]
timeout_hours = -1
[isolation.container]
image = "runner:latest"
network = "host"
user = "root;rm"
[auth]
mode = "proxy"
"#;
        let e = cfg_from(text).unwrap_err().to_string();
        for needle in [
            "server.listen",
            "server.public_url",
            "trusted_proxies[1]",
            "max_body_bytes",
            "timeout_hours",
            "image",
            "network",
            "container.user",
        ] {
            assert!(e.contains(needle), "missing {needle} in {e}");
        }
    }

    #[test]
    fn proxy_mode_needs_trusted_proxies() {
        let e = cfg_from("[auth]\nmode = \"proxy\"\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("trusted_proxies"), "{e}");
        cfg_from("[auth]\nmode = \"proxy\"\n[server]\ntrusted_proxies = [\"127.0.0.1\"]\n")
            .unwrap();
    }

    #[test]
    fn images_must_be_pinned() {
        assert!(image_is_pinned("harness2-studio-runner:0.1.0"));
        assert!(image_is_pinned("registry.example.com:5000/runner:1.2"));
        assert!(image_is_pinned(&format!(
            "runner@sha256:{}",
            "a".repeat(64)
        )));
        for bad in [
            "runner",
            "runner:latest",
            "registry.example.com:5000/runner",
            "runner@sha256:abc",
            "-runner:1",
            "run ner:1",
            "Runner:1",
            "",
        ] {
            assert!(!image_is_pinned(bad), "{bad}");
        }
    }

    #[test]
    fn networks_and_users() {
        assert!(network_ok("none"));
        assert!(network_ok("h2s-egress"));
        for bad in ["host", "bridge", "container:x", "-x", ""] {
            assert!(!network_ok(bad), "{bad}");
        }
        for ok in ["1000", "1000:1000", "0:0"] {
            assert!(container_user_ok(ok), "{ok}");
        }
        for bad in ["1000:", ":1", "Root", "runner", "a b", "1:2:3", ""] {
            assert!(!container_user_ok(bad), "{bad}");
        }
    }

    #[test]
    fn container_proxy_urls() {
        for ok in [
            "http://egress:3128",
            "http://egress:3128/",
            "http://10.0.0.2:3128",
            "http://[fd00::2]:3128",
        ] {
            assert!(proxy_url_ok(ok), "{ok}");
        }
        for bad in [
            "egress:3128",
            "https://egress:3128",
            "http://egress",
            "http://egress:0",
            "http://user:pass@egress:3128",
            "http://egress:3128/path",
            "http://egress:3128?x=1",
            "http://egr ess:3128",
            "http://",
            "",
        ] {
            assert!(!proxy_url_ok(bad), "{bad}");
        }
    }

    #[test]
    fn egress_entries() {
        // IP literals are refused before any core validation runs.
        let e = cfg_from("[egress]\nallow = [\"10.0.0.1\"]\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("IP literals"), "{e}");
        let e = cfg_from("[egress]\nallow = [{host = \"x\", allow_privat = true}]\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("egress") || e.contains("allow"), "{e}");
    }

    #[test]
    fn egress_hostnames_are_validated_by_core() {
        let c = cfg_from(
            "[egress]\nallow = [\"aiplatform.googleapis.com\", \"*.googleapis.com\", {host = \"litellm\", allow_private = true}]\n",
        )
        .unwrap();
        assert_eq!(c.egress_allow.len(), 3);
        assert!(c.egress_allow[1].wildcard);
        assert!(c.egress_allow[2].allow_private);
        assert!(cfg_from("[egress]\nallow = [\"bad host\"]\n").is_err());
    }

    #[test]
    fn host_entries() {
        let e = HostEntry::parse("Studio.Example.com:8443").unwrap();
        assert_eq!(e.host, "studio.example.com");
        assert_eq!(e.port, Some(8443));
        assert!(e.matches("studio.example.com", Some(8443)));
        assert!(!e.matches("studio.example.com", Some(80)));
        let any = HostEntry::parse("example.org").unwrap();
        assert!(any.matches("example.org", Some(1)));
        assert!(any.matches("example.org", None));
        assert_eq!(
            split_host_port("[::1]:8722"),
            Some(("::1".to_owned(), Some(8722)))
        );
        assert_eq!(split_host_port("::1"), None);
        assert_eq!(split_host_port("a:b"), None);
        assert_eq!(split_host_port("a b"), None);
        assert_eq!(split_host_port(""), None);
    }

    #[test]
    fn initial_config_round_trips() {
        let text = render_initial(Path::new("/srv/h2s")).unwrap();
        let c = cfg_from(&text).unwrap();
        assert_eq!(c.paths.data_dir, PathBuf::from("/srv/h2s"));
    }

    proptest! {
        #[test]
        fn split_host_port_never_panics(s in ".{0,64}") {
            let _ = split_host_port(&s);
            let _ = HostEntry::parse(&s);
        }

        #[test]
        fn split_host_port_round_trips(host in "[a-z][a-z0-9-]{0,20}(\\.[a-z][a-z0-9-]{0,10}){0,3}", port in proptest::option::of(any::<u16>())) {
            let s = match port { Some(p) => format!("{host}:{p}"), None => host.clone() };
            prop_assert_eq!(split_host_port(&s), Some((host, port)));
        }

        #[test]
        fn image_check_never_panics(s in ".{0,80}") {
            let _ = image_is_pinned(&s);
            let _ = network_ok(&s);
            let _ = container_user_ok(&s);
        }
    }
}
