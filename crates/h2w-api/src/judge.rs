//! Where a profile's judge runs: on infrastructure the operator controls, or at a cloud
//! provider.
//!
//! Judging sends each task's instructions, its reference material and the agent's
//! deliverables to the judge model. A cloud judge should only see generated or synthetic
//! data, so the server, the web UI and the MCP server all classify a profile's judge the
//! same way, with [`judge_site`], and warn with [`cloud_judge_warning`]. The server
//! refuses judge and pipeline jobs with a cloud judge unless the caller confirms the data
//! is synthetic (`acknowledge_cloud_judge`), or refuses them outright when an administrator
//! set `[judge] cloud = "refuse"`.
//!
//! The classification looks at host names only. A gateway on the operator's own network
//! that forwards to a cloud provider counts as local; an internal gateway with a public DNS
//! name counts as cloud unless it is listed in `[judge] private_hosts`.

use core::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use serde::{Deserialize, Serialize};

use crate::{ModelProfile, ProfileKind};

/// Where a profile's judge runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum JudgeSiteKind {
    /// Every host judging reaches is loopback, a private address, a single-label name (a
    /// container or service name), a local-only domain or listed in `[judge] private_hosts`.
    Local,
    /// At least one host judging reaches is on the public internet.
    Cloud,
    /// The judge's endpoint is set through a secret or not at all, so the server cannot
    /// tell; jobs treat it as a cloud judge.
    Unknown,
}

/// Where a profile's judge sends what it grades. Computed by the server (and the web UI)
/// from the profile; ignored when a client sends it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct JudgeSite {
    pub kind: JudgeSiteKind,
    /// The cloud provider, when a host is a known one (`Anthropic`, `OpenAI`,
    /// `Google Vertex AI`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// The hosts judging reaches, lower-case, without ports.
    #[serde(default)]
    pub hosts: Vec<String>,
}

impl JudgeSite {
    /// Judge and pipeline jobs need `acknowledge_cloud_judge` (or are refused by policy).
    #[must_use]
    pub fn needs_acknowledgement(&self) -> bool {
        self.kind != JudgeSiteKind::Local
    }

    /// Who receives the data, for messages: the provider with its hosts, or the hosts.
    #[must_use]
    pub fn describe(&self) -> String {
        let hosts = self.hosts.join(", ");
        match (&self.provider, hosts.is_empty()) {
            (Some(p), false) => format!("{p}: {hosts}"),
            (Some(p), true) => p.clone(),
            (None, false) => hosts,
            (None, true) => "an endpoint the server cannot see".to_owned(),
        }
    }
}

/// How the server treats judge and pipeline jobs whose profile has a cloud judge
/// (`[judge] cloud`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum CloudJudgePolicy {
    /// Such jobs need `acknowledge_cloud_judge: true`: the caller confirms that the
    /// experiment's tasks and outputs are generated or synthetic data.
    #[default]
    Acknowledge,
    /// Such jobs are refused.
    Refuse,
}

/// The rule every cloud-judge warning ends with.
pub const CLOUD_JUDGE_RULE: &str = "Use a cloud judge only with generated or synthetic data, \
     never with private, confidential or customer data.";

/// The warning for a judge that is not local (empty for a local one).
#[must_use]
pub fn cloud_judge_warning(site: &JudgeSite) -> String {
    match site.kind {
        JudgeSiteKind::Local => String::new(),
        JudgeSiteKind::Cloud => format!(
            "This profile's judge is a cloud service ({}). Judging sends each task's \
             instructions, its reference files and the agent's deliverables there, to be \
             stored and processed under that provider's terms. {CLOUD_JUDGE_RULE}",
            site.describe()
        ),
        JudgeSiteKind::Unknown => format!(
            "The Workbench cannot tell where this profile's judge runs: its endpoint is set \
             through a secret or not at all. Judge and pipeline jobs treat it as a cloud \
             service. {CLOUD_JUDGE_RULE}"
        ),
    }
}

/// Where the judge of `profile` runs, or `None` when the profile has no judge model.
///
/// It follows how the server builds a job's environment (`h2w_core::env`) and how harness2
/// judges at the tested commit:
/// - the judge endpoint, or for `openai_compatible` profiles the main endpoint when no judge
///   endpoint is set, is what JobBench's judge and LAB's `gpt*`/`o*` judges call;
/// - `claude*`, `gemini*` and `vertex/` judge ids go to Vertex AI when the profile has
///   Vertex settings (LAB routes them there; JobBench's Gemini judge too);
/// - `extra_env` literals for `HARNESS2_JUDGE_BASE_URL`, `OPENAI_BASE_URL` and
///   `ANTHROPIC_BASE_URL` count as well; the same names set from a secret make it unknown.
///
/// It errs towards "cloud": every candidate host counts, even one another setting would
/// override.
#[must_use]
pub fn judge_site(profile: &ModelProfile, private_hosts: &[String]) -> Option<JudgeSite> {
    let literal = |name: &str| {
        profile
            .extra_env
            .iter()
            .find(|e| e.name == name)
            .map(|e| (e.value.clone(), e.secret.is_some()))
    };
    let judge = match &profile.judge_model {
        Some(j) => j.clone(),
        None => match literal("HARNESS2_JUDGE_MODEL") {
            Some((Some(v), _)) => v,
            Some((None, true)) => String::new(),
            _ => return None,
        },
    };
    let mut urls: Vec<String> = Vec::new();
    let mut unknown = judge.is_empty();
    match profile.kind {
        ProfileKind::OpenaiCompatible => {
            if let Some(ep) = profile
                .judge_endpoint
                .as_ref()
                .or(profile.endpoint.as_ref())
            {
                urls.push(ep.base_url.clone());
            }
        }
        ProfileKind::Vertex | ProfileKind::Custom => {
            if let Some(ep) = &profile.judge_endpoint {
                urls.push(ep.base_url.clone());
            }
        }
    }
    for name in [
        "HARNESS2_JUDGE_BASE_URL",
        "OPENAI_BASE_URL",
        "ANTHROPIC_BASE_URL",
    ] {
        match literal(name) {
            Some((Some(v), _)) => urls.push(v),
            Some((None, true)) => unknown = true,
            _ => {}
        }
    }
    let mut hosts: Vec<String> = Vec::new();
    for u in &urls {
        match url_host(u) {
            Some(h) => push_unique(&mut hosts, h),
            None => unknown = true,
        }
    }
    let vertex_judge = ["claude", "gemini", "vertex/"]
        .iter()
        .any(|p| judge.starts_with(p));
    if vertex_judge {
        let location = match (&profile.vertex, literal("GOOGLE_CLOUD_PROJECT")) {
            (Some(v), _) => Some(v.location.clone()),
            (None, Some(_)) => Some(match literal("GOOGLE_CLOUD_LOCATION") {
                Some((Some(l), _)) => l,
                _ => "global".to_owned(),
            }),
            (None, None) => None,
        };
        if let Some(loc) = location {
            push_unique(&mut hosts, vertex_host(&loc));
        }
    }
    if hosts.is_empty() {
        unknown = true;
    }
    let cloud: Vec<&String> = hosts
        .iter()
        .filter(|h| !is_private_host(h, private_hosts))
        .collect();
    let kind = if !cloud.is_empty() {
        JudgeSiteKind::Cloud
    } else if unknown {
        JudgeSiteKind::Unknown
    } else {
        JudgeSiteKind::Local
    };
    let provider = cloud
        .iter()
        .find_map(|h| provider_name(h))
        .map(str::to_owned);
    Some(JudgeSite {
        kind,
        provider,
        hosts,
    })
}

fn push_unique(hosts: &mut Vec<String>, h: String) {
    if !hosts.contains(&h) {
        hosts.push(h);
    }
}

/// Vertex AI's host for a location, as `h2w_core::env` derives it.
fn vertex_host(location: &str) -> String {
    if location == "global" {
        "aiplatform.googleapis.com".to_owned()
    } else {
        format!(
            "{}-aiplatform.googleapis.com",
            location.to_ascii_lowercase()
        )
    }
}

/// The host of an `http(s)` URL: lower-case, without brackets, port or a trailing dot.
#[must_use]
pub fn url_host(url: &str) -> Option<String> {
    let lower = url.trim().to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit_once('@').map_or(authority, |(_, a)| a);
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split_once(']')?.0
    } else {
        authority.split(':').next()?
    };
    let host = host.trim_end_matches('.');
    if host.is_empty() {
        None
    } else {
        Some(host.to_owned())
    }
}

/// Whether a host is on the operator's own infrastructure: loopback, a private, link-local
/// or shared (CGNAT) address, a single-label name, a local-only domain (`localhost`,
/// `.local`, `.internal`, `.lan`, `.home.arpa`), or an entry of `private_hosts` (a host
/// name, or `.example.com` for a whole domain).
#[must_use]
pub fn is_private_host(host: &str, private_hosts: &[String]) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if private_hosts.iter().any(|p| {
        let p = p.trim().trim_end_matches('.').to_ascii_lowercase();
        match p.strip_prefix('.') {
            Some(domain) => host == domain || host.ends_with(&format!(".{domain}")),
            None => !p.is_empty() && host == p,
        }
    }) {
        return true;
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return match ip {
            IpAddr::V4(v4) => private_v4(v4),
            IpAddr::V6(v6) => private_v6(v6),
        };
    }
    if !host.contains('.') {
        return true;
    }
    [".localhost", ".local", ".internal", ".lan", ".home.arpa"]
        .iter()
        .any(|s| host.ends_with(s))
}

fn private_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || (a == 100 && (64..128).contains(&b))
}

fn private_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return private_v4(v4);
    }
    let first = ip.segments()[0];
    ip.is_loopback()
        || ip.is_unspecified()
        || (first & 0xfe00) == 0xfc00
        || (first & 0xffc0) == 0xfe80
}

/// The provider behind a well-known API host.
#[must_use]
pub fn provider_name(host: &str) -> Option<&'static str> {
    let h = host.to_ascii_lowercase();
    let is = |d: &str| h == d || h.ends_with(&format!(".{d}"));
    Some(if is("anthropic.com") {
        "Anthropic"
    } else if is("openai.com") {
        "OpenAI"
    } else if is("azure.com") {
        "Microsoft Azure"
    } else if h == "generativelanguage.googleapis.com" {
        "Google Gemini API"
    } else if h == "aiplatform.googleapis.com" || h.ends_with("-aiplatform.googleapis.com") {
        "Google Vertex AI"
    } else if is("googleapis.com") {
        "Google Cloud"
    } else if is("amazonaws.com") {
        "Amazon Web Services"
    } else if is("openrouter.ai") {
        "OpenRouter"
    } else if is("mistral.ai") {
        "Mistral AI"
    } else if is("x.ai") {
        "xAI"
    } else if is("deepseek.com") {
        "DeepSeek"
    } else if is("together.xyz") || is("together.ai") {
        "Together AI"
    } else if is("fireworks.ai") {
        "Fireworks AI"
    } else if is("groq.com") {
        "Groq"
    } else if is("nvidia.com") {
        "NVIDIA"
    } else {
        return None;
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Endpoint, EnvVar, VertexSettings};

    fn oai(endpoint: &str, judge_endpoint: Option<&str>, judge: &str) -> ModelProfile {
        ModelProfile {
            name: "p".into(),
            kind: ProfileKind::OpenaiCompatible,
            description: String::new(),
            solver_model: "m".into(),
            improver_model: None,
            judge_model: Some(judge.into()),
            codex_model: None,
            vertex: None,
            endpoint: Some(Endpoint {
                base_url: endpoint.into(),
                api_key_secret: None,
            }),
            judge_endpoint: judge_endpoint.map(|u| Endpoint {
                base_url: u.into(),
                api_key_secret: Some("JUDGE_KEY".into()),
            }),
            codex_endpoint: None,
            extra_env: vec![],
            prices: None,
            egress_hosts: vec![],
            secrets: vec![],
            judge_site: None,
        }
    }

    #[test]
    fn a_router_on_the_jobs_network_is_local() {
        let s = judge_site(&oai("http://litellm:4000/v1", None, "gpt-judge"), &[]).unwrap();
        assert_eq!(s.kind, JudgeSiteKind::Local);
        assert_eq!(s.hosts, ["litellm"]);
        assert!(!s.needs_acknowledgement());
        assert!(cloud_judge_warning(&s).is_empty());
    }

    #[test]
    fn anthropic_is_cloud_and_named() {
        let p = oai(
            "http://127.0.0.1:4000/v1",
            Some("https://api.anthropic.com/v1/"),
            "claude-sonnet-4-6",
        );
        let s = judge_site(&p, &[]).unwrap();
        assert_eq!(s.kind, JudgeSiteKind::Cloud);
        assert_eq!(s.provider.as_deref(), Some("Anthropic"));
        assert_eq!(s.hosts, ["api.anthropic.com"]);
        let w = cloud_judge_warning(&s);
        assert!(w.contains("Anthropic: api.anthropic.com"), "{w}");
        assert!(w.ends_with(CLOUD_JUDGE_RULE), "{w}");
    }

    #[test]
    fn the_main_endpoint_judges_when_there_is_no_judge_endpoint() {
        let s = judge_site(&oai("https://api.openai.com/v1", None, "gpt-5-mini"), &[]).unwrap();
        assert_eq!(s.kind, JudgeSiteKind::Cloud);
        assert_eq!(s.provider.as_deref(), Some("OpenAI"));
    }

    #[test]
    fn no_judge_model_means_no_site() {
        let mut p = oai("http://litellm:4000/v1", None, "x");
        p.judge_model = None;
        assert!(judge_site(&p, &[]).is_none());
    }

    #[test]
    fn vertex_settings_send_claude_and_gemini_judges_to_vertex() {
        let mut p = oai("http://litellm:4000/v1", None, "claude-sonnet-4-6");
        p.vertex = Some(VertexSettings {
            project: "proj".into(),
            location: "us-east5".into(),
            credentials_secret: None,
        });
        let s = judge_site(&p, &[]).unwrap();
        assert_eq!(s.kind, JudgeSiteKind::Cloud);
        assert_eq!(s.hosts, ["litellm", "us-east5-aiplatform.googleapis.com"]);
        assert_eq!(s.provider.as_deref(), Some("Google Vertex AI"));
        // A gpt* judge does not go to Vertex.
        p.judge_model = Some("gpt-judge".into());
        assert_eq!(judge_site(&p, &[]).unwrap().kind, JudgeSiteKind::Local);
    }

    #[test]
    fn private_addresses_and_names_are_local() {
        for h in [
            "127.0.0.1",
            "10.1.2.3",
            "172.20.0.5",
            "192.168.1.20",
            "169.254.0.1",
            "100.100.1.1",
            "::1",
            "fd00::5",
            "fe80::1",
            "::ffff:10.0.0.1",
            "localhost",
            "gpu.localhost",
            "litellm",
            "box.local",
            "llm.internal",
            "router.lan",
            "nas.home.arpa",
        ] {
            assert!(is_private_host(h, &[]), "{h}");
        }
        for h in [
            "8.8.8.8",
            "100.128.0.1",
            "2001:4860::8888",
            "api.anthropic.com",
            "llm.corp.example.com",
        ] {
            assert!(!is_private_host(h, &[]), "{h}");
        }
    }

    #[test]
    fn private_hosts_mark_internal_gateways_local() {
        let p = oai("https://llm.corp.example.com/v1", None, "gpt-judge");
        assert_eq!(judge_site(&p, &[]).unwrap().kind, JudgeSiteKind::Cloud);
        let exact = vec!["LLM.corp.example.com".to_owned()];
        assert_eq!(judge_site(&p, &exact).unwrap().kind, JudgeSiteKind::Local);
        let domain = vec![".corp.example.com".to_owned()];
        assert_eq!(judge_site(&p, &domain).unwrap().kind, JudgeSiteKind::Local);
        let other = vec![".example.org".to_owned(), String::new()];
        assert_eq!(judge_site(&p, &other).unwrap().kind, JudgeSiteKind::Cloud);
    }

    #[test]
    fn endpoints_set_through_secrets_are_unknown() {
        let mut p = oai("http://litellm:4000/v1", None, "gpt-judge");
        p.kind = ProfileKind::Custom;
        p.endpoint = None;
        p.extra_env = vec![EnvVar {
            name: "HARNESS2_JUDGE_BASE_URL".into(),
            value: None,
            secret: Some("JUDGE_URL".into()),
        }];
        let s = judge_site(&p, &[]).unwrap();
        assert_eq!(s.kind, JudgeSiteKind::Unknown);
        assert!(s.needs_acknowledgement());
        assert!(cloud_judge_warning(&s).contains("cannot tell"));
        // A custom profile with a literal local URL is local; a cloud one is cloud.
        p.extra_env[0] = EnvVar {
            name: "HARNESS2_JUDGE_BASE_URL".into(),
            value: Some("http://10.0.0.9:8000/v1".into()),
            secret: None,
        };
        assert_eq!(judge_site(&p, &[]).unwrap().kind, JudgeSiteKind::Local);
        p.extra_env.push(EnvVar {
            name: "OPENAI_BASE_URL".into(),
            value: Some("https://openrouter.ai/api/v1".into()),
            secret: None,
        });
        let s = judge_site(&p, &[]).unwrap();
        assert_eq!(
            (s.kind, s.provider.as_deref()),
            (JudgeSiteKind::Cloud, Some("OpenRouter"))
        );
    }

    #[test]
    fn custom_profiles_take_the_judge_model_from_extra_env() {
        let mut p = oai("http://litellm:4000/v1", None, "x");
        p.kind = ProfileKind::Custom;
        p.judge_model = None;
        p.endpoint = None;
        assert!(judge_site(&p, &[]).is_none());
        p.extra_env = vec![
            EnvVar {
                name: "HARNESS2_JUDGE_MODEL".into(),
                value: Some("gemini-3-pro".into()),
                secret: None,
            },
            EnvVar {
                name: "GOOGLE_CLOUD_PROJECT".into(),
                value: Some("proj".into()),
                secret: None,
            },
        ];
        let s = judge_site(&p, &[]).unwrap();
        assert_eq!(s.hosts, ["aiplatform.googleapis.com"]);
        assert_eq!(s.kind, JudgeSiteKind::Cloud);
        // No endpoint at all: unknown, not local.
        p.extra_env.truncate(1);
        assert_eq!(judge_site(&p, &[]).unwrap().kind, JudgeSiteKind::Unknown);
    }

    #[test]
    fn hosts_are_parsed_strictly_enough() {
        assert_eq!(
            url_host("https://API.Anthropic.com/v1/").as_deref(),
            Some("api.anthropic.com")
        );
        assert_eq!(url_host("http://[::1]:4000/v1").as_deref(), Some("::1"));
        assert_eq!(url_host("http://litellm:4000").as_deref(), Some("litellm"));
        assert_eq!(
            url_host("https://user@example.com./x").as_deref(),
            Some("example.com")
        );
        assert_eq!(url_host("ftp://example.com"), None);
        assert_eq!(url_host("https:///v1"), None);
        // An unparsable judge URL is unknown, never local.
        let p = oai("not a url", None, "gpt-judge");
        assert_eq!(judge_site(&p, &[]).unwrap().kind, JudgeSiteKind::Unknown);
    }

    #[test]
    fn providers_are_named_by_domain() {
        assert_eq!(provider_name("api.anthropic.com"), Some("Anthropic"));
        assert_eq!(
            provider_name("myres.openai.azure.com"),
            Some("Microsoft Azure")
        );
        assert_eq!(
            provider_name("europe-west4-aiplatform.googleapis.com"),
            Some("Google Vertex AI")
        );
        assert_eq!(
            provider_name("generativelanguage.googleapis.com"),
            Some("Google Gemini API")
        );
        assert_eq!(
            provider_name("bedrock-runtime.us-east-1.amazonaws.com"),
            Some("Amazon Web Services")
        );
        assert_eq!(provider_name("notanthropic.com"), None);
        assert_eq!(provider_name("llm.corp.example.com"), None);
    }
}
