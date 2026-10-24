//! The model-profile editor: a text form that round-trips `ModelProfile`.

use h2w_api::{Endpoint, EnvVar, ModelProfile, Prices, ProfileKind, VertexSettings};

use super::names;

pub(crate) type Problems = Vec<(String, String)>;

pub(crate) const KINDS: [ProfileKind; 3] = [
    ProfileKind::Vertex,
    ProfileKind::OpenaiCompatible,
    ProfileKind::Custom,
];

pub(crate) fn kind_label(k: ProfileKind) -> &'static str {
    match k {
        ProfileKind::Vertex => "Google Vertex AI",
        ProfileKind::OpenaiCompatible => "OpenAI-compatible endpoint",
        ProfileKind::Custom => "Custom environment",
    }
}

pub(crate) fn kind_value(k: ProfileKind) -> &'static str {
    match k {
        ProfileKind::Vertex => "vertex",
        ProfileKind::OpenaiCompatible => "openai_compatible",
        ProfileKind::Custom => "custom",
    }
}

pub(crate) fn kind_from_value(s: &str) -> ProfileKind {
    match s {
        "openai_compatible" => ProfileKind::OpenaiCompatible,
        "custom" => ProfileKind::Custom,
        _ => ProfileKind::Vertex,
    }
}

pub(crate) fn kind_help(k: ProfileKind) -> &'static str {
    match k {
        ProfileKind::Vertex => {
            "Upstream's configuration: Gemini models on Vertex AI with Application Default \
             Credentials, or a service-account key stored as a secret."
        }
        ProfileKind::OpenaiCompatible => {
            "Any OpenAI-compatible server (LiteLLM, vLLM, SGLang, Ollama or a hosted gateway). \
             The Workbench wires it into OpenCode as a custom provider and into the judges."
        }
        ProfileKind::Custom => {
            "Raw harness2 environment variables, for setups the other kinds do not cover."
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct EnvRow {
    pub(crate) name: String,
    /// Literal value (ignored when `from_secret`).
    pub(crate) value: String,
    pub(crate) secret: String,
    pub(crate) from_secret: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProfileForm {
    pub(crate) name: String,
    pub(crate) kind: ProfileKind,
    pub(crate) description: String,
    pub(crate) solver_model: String,
    pub(crate) improver_model: String,
    pub(crate) judge_model: String,
    pub(crate) codex_model: String,
    pub(crate) vertex_project: String,
    pub(crate) vertex_location: String,
    pub(crate) vertex_credentials_secret: String,
    pub(crate) endpoint_url: String,
    pub(crate) endpoint_secret: String,
    pub(crate) judge_url: String,
    pub(crate) judge_secret: String,
    pub(crate) codex_url: String,
    pub(crate) codex_secret: String,
    pub(crate) extra_env: Vec<EnvRow>,
    pub(crate) prices: bool,
    pub(crate) rollout_low: String,
    pub(crate) rollout_high: String,
    pub(crate) session_low: String,
    pub(crate) session_high: String,
    /// Hosts separated by commas, spaces or new lines.
    pub(crate) egress_hosts: String,
}

impl Default for ProfileForm {
    fn default() -> Self {
        ProfileForm {
            name: String::new(),
            kind: ProfileKind::OpenaiCompatible,
            description: String::new(),
            solver_model: String::new(),
            improver_model: String::new(),
            judge_model: String::new(),
            codex_model: String::new(),
            vertex_project: String::new(),
            vertex_location: "global".to_owned(),
            vertex_credentials_secret: String::new(),
            endpoint_url: String::new(),
            endpoint_secret: String::new(),
            judge_url: String::new(),
            judge_secret: String::new(),
            codex_url: String::new(),
            codex_secret: String::new(),
            extra_env: Vec::new(),
            prices: false,
            rollout_low: String::new(),
            rollout_high: String::new(),
            session_low: String::new(),
            session_high: String::new(),
            egress_hosts: String::new(),
        }
    }
}

fn opt(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

fn money(v: f64) -> String {
    format!("{v}")
}

fn split_endpoint(e: Option<&Endpoint>) -> (String, String) {
    e.map(|e| {
        (
            e.base_url.clone(),
            e.api_key_secret.clone().unwrap_or_default(),
        )
    })
    .unwrap_or_default()
}

impl ProfileForm {
    pub(crate) fn from_profile(p: &ModelProfile) -> ProfileForm {
        let (endpoint_url, endpoint_secret) = split_endpoint(p.endpoint.as_ref());
        let (judge_url, judge_secret) = split_endpoint(p.judge_endpoint.as_ref());
        let (codex_url, codex_secret) = split_endpoint(p.codex_endpoint.as_ref());
        let vertex = p.vertex.clone();
        ProfileForm {
            name: p.name.clone(),
            kind: p.kind,
            description: p.description.clone(),
            solver_model: p.solver_model.clone(),
            improver_model: p.improver_model.clone().unwrap_or_default(),
            judge_model: p.judge_model.clone().unwrap_or_default(),
            codex_model: p.codex_model.clone().unwrap_or_default(),
            vertex_project: vertex
                .as_ref()
                .map(|v| v.project.clone())
                .unwrap_or_default(),
            vertex_location: vertex
                .as_ref()
                .map_or_else(|| "global".to_owned(), |v| v.location.clone()),
            vertex_credentials_secret: vertex
                .and_then(|v| v.credentials_secret)
                .unwrap_or_default(),
            endpoint_url,
            endpoint_secret,
            judge_url,
            judge_secret,
            codex_url,
            codex_secret,
            extra_env: p
                .extra_env
                .iter()
                .map(|e| EnvRow {
                    name: e.name.clone(),
                    value: e.value.clone().unwrap_or_default(),
                    secret: e.secret.clone().unwrap_or_default(),
                    from_secret: e.secret.is_some(),
                })
                .collect(),
            prices: p.prices.is_some(),
            rollout_low: p
                .prices
                .as_ref()
                .map(|x| money(x.rollout_usd_low))
                .unwrap_or_default(),
            rollout_high: p
                .prices
                .as_ref()
                .map(|x| money(x.rollout_usd_high))
                .unwrap_or_default(),
            session_low: p
                .prices
                .as_ref()
                .map(|x| money(x.session_usd_low))
                .unwrap_or_default(),
            session_high: p
                .prices
                .as_ref()
                .map(|x| money(x.session_usd_high))
                .unwrap_or_default(),
            egress_hosts: p.egress_hosts.join("\n"),
        }
    }

    fn endpoint(
        &self,
        field: &str,
        url: &str,
        secret: &str,
        problems: &mut Problems,
    ) -> Option<Endpoint> {
        let url = opt(url)?;
        if let Err(e) = names::base_url(&url) {
            problems.push((field.to_owned(), e));
        }
        let secret = opt(secret);
        if let Some(s) = &secret
            && let Err(e) = names::secret_name(s)
        {
            problems.push((format!("{field}_secret"), e));
        }
        Some(Endpoint {
            base_url: url,
            api_key_secret: secret,
        })
    }

    /// Build the profile to `PUT`, or every problem found.
    pub(crate) fn to_profile(&self) -> Result<ModelProfile, Problems> {
        let mut p = Problems::new();
        if let Err(e) = names::profile_name(self.name.trim()) {
            p.push(("name".into(), e));
        }
        if let Err(e) = names::model_id(self.solver_model.trim()) {
            p.push(("solver_model".into(), e));
        }
        for (field, value) in [
            ("improver_model", &self.improver_model),
            ("judge_model", &self.judge_model),
            ("codex_model", &self.codex_model),
        ] {
            if let Some(v) = opt(value)
                && let Err(e) = names::model_id(&v)
            {
                p.push((field.into(), e));
            }
        }
        let vertex = if self.kind == ProfileKind::Vertex {
            if self.vertex_project.trim().is_empty() {
                p.push((
                    "vertex_project".into(),
                    "Vertex profiles need a Google Cloud project id.".into(),
                ));
            }
            let creds = opt(&self.vertex_credentials_secret);
            if let Some(c) = &creds
                && let Err(e) = names::secret_name(c)
            {
                p.push(("vertex_credentials_secret".into(), e));
            }
            Some(VertexSettings {
                project: self.vertex_project.trim().to_owned(),
                location: opt(&self.vertex_location).unwrap_or_else(|| "global".to_owned()),
                credentials_secret: creds,
            })
        } else {
            None
        };
        let endpoint = self.endpoint(
            "endpoint",
            &self.endpoint_url,
            &self.endpoint_secret,
            &mut p,
        );
        if self.kind == ProfileKind::OpenaiCompatible && endpoint.is_none() {
            p.push((
                "endpoint".into(),
                "OpenAI-compatible profiles need the endpoint's base URL.".into(),
            ));
        }
        let judge_endpoint = self.endpoint("judge", &self.judge_url, &self.judge_secret, &mut p);
        let codex_endpoint = self.endpoint("codex", &self.codex_url, &self.codex_secret, &mut p);
        let mut extra_env = Vec::new();
        for (i, row) in self.extra_env.iter().enumerate() {
            let field = format!("extra_env[{i}]");
            let name = row.name.trim();
            if name.is_empty() && row.value.is_empty() && row.secret.is_empty() {
                continue;
            }
            if let Err(e) = names::env_name(name) {
                p.push((field.clone(), e));
            }
            if row.from_secret {
                if let Err(e) = names::secret_name(row.secret.trim()) {
                    p.push((field, e));
                }
                extra_env.push(EnvVar {
                    name: name.to_owned(),
                    value: None,
                    secret: Some(row.secret.trim().to_owned()),
                });
            } else {
                extra_env.push(EnvVar {
                    name: name.to_owned(),
                    value: Some(row.value.clone()),
                    secret: None,
                });
            }
        }
        let prices = if self.prices {
            let mut num = |field: &str, s: &str| -> f64 {
                match s.trim().trim_start_matches('$').parse::<f64>() {
                    Ok(v) if v.is_finite() && v >= 0.0 => v,
                    _ => {
                        p.push((field.to_owned(), "enter an amount in US dollars".into()));
                        0.0
                    }
                }
            };
            let prices = Prices {
                rollout_usd_low: num("rollout_low", &self.rollout_low),
                rollout_usd_high: num("rollout_high", &self.rollout_high),
                session_usd_low: num("session_low", &self.session_low),
                session_usd_high: num("session_high", &self.session_high),
            };
            if prices.rollout_usd_low > prices.rollout_usd_high
                || prices.session_usd_low > prices.session_usd_high
            {
                p.push((
                    "prices".into(),
                    "each low price must not exceed its high price".into(),
                ));
            }
            Some(prices)
        } else {
            None
        };
        let mut egress_hosts = Vec::new();
        for h in self
            .egress_hosts
            .split([',', ' ', '\n', '\t'])
            .map(str::trim)
            .filter(|h| !h.is_empty())
        {
            if let Err(e) = names::hostname(h) {
                p.push(("egress_hosts".into(), e));
            } else if !egress_hosts.iter().any(|x: &String| x == h) {
                egress_hosts.push(h.to_owned());
            }
        }
        if !p.is_empty() {
            return Err(p);
        }
        Ok(ModelProfile {
            name: self.name.trim().to_owned(),
            kind: self.kind,
            description: self.description.trim().to_owned(),
            solver_model: self.solver_model.trim().to_owned(),
            improver_model: opt(&self.improver_model),
            judge_model: opt(&self.judge_model),
            codex_model: opt(&self.codex_model),
            vertex,
            endpoint,
            judge_endpoint,
            codex_endpoint,
            extra_env,
            prices,
            egress_hosts,
            secrets: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use h2w_api::SecretInfo;

    fn local() -> ProfileForm {
        ProfileForm {
            name: "local-glm".into(),
            solver_model: "glm-5".into(),
            endpoint_url: "http://127.0.0.1:4000/v1".into(),
            endpoint_secret: "LITELLM_KEY".into(),
            ..ProfileForm::default()
        }
    }

    #[test]
    fn builds_openai_compatible_profile() {
        let p = local().to_profile().unwrap();
        assert_eq!(p.kind, ProfileKind::OpenaiCompatible);
        assert_eq!(
            p.endpoint.as_ref().unwrap().api_key_secret.as_deref(),
            Some("LITELLM_KEY")
        );
        assert!(p.vertex.is_none());
        assert!(p.improver_model.is_none());
    }

    #[test]
    fn reports_every_problem() {
        let f = ProfileForm {
            name: "Bad Name".into(),
            solver_model: String::new(),
            endpoint_url: "localhost:4000".into(),
            endpoint_secret: "lower".into(),
            egress_hosts: "ok.example.com, BAD_HOST".into(),
            extra_env: vec![EnvRow {
                name: "LD_PRELOAD".into(),
                value: "x".into(),
                ..EnvRow::default()
            }],
            prices: true,
            rollout_low: "2".into(),
            rollout_high: "1".into(),
            session_low: "x".into(),
            session_high: "1".into(),
            ..ProfileForm::default()
        };
        let fields: Vec<String> = f
            .to_profile()
            .unwrap_err()
            .into_iter()
            .map(|(f, _)| f)
            .collect();
        for want in [
            "name",
            "solver_model",
            "endpoint",
            "endpoint_secret",
            "egress_hosts",
            "extra_env[0]",
            "session_low",
            "prices",
        ] {
            assert!(
                fields.iter().any(|f| f == want),
                "missing {want}: {fields:?}"
            );
        }
    }

    #[test]
    fn vertex_needs_a_project() {
        let mut f = ProfileForm {
            kind: ProfileKind::Vertex,
            name: "vertex".into(),
            solver_model: "google-vertex/gemini-3.5-flash".into(),
            ..ProfileForm::default()
        };
        assert!(f.to_profile().is_err());
        f.vertex_project = "my-project".into();
        let p = f.to_profile().unwrap();
        assert_eq!(p.vertex.unwrap().location, "global");
    }

    #[test]
    fn round_trips() {
        let mut f = local();
        f.extra_env = vec![
            EnvRow {
                name: "HARNESS2_EXTRA".into(),
                value: "1".into(),
                ..EnvRow::default()
            },
            EnvRow {
                name: "MY_TOKEN".into(),
                secret: "MY_TOKEN".into(),
                from_secret: true,
                ..EnvRow::default()
            },
            EnvRow::default(),
        ];
        f.prices = true;
        f.rollout_low = "0.05".into();
        f.rollout_high = "0.2".into();
        f.session_low = "0.1".into();
        f.session_high = "0.4".into();
        f.egress_hosts = "a.example.com\n*.b.example.com a.example.com".into();
        let mut p = f.to_profile().unwrap();
        assert_eq!(p.extra_env.len(), 2);
        assert_eq!(p.egress_hosts.len(), 2);
        p.secrets = vec![SecretInfo {
            name: "LITELLM_KEY".into(),
            set: true,
            chars: Some(40),
            updated_at: None,
        }];
        let back = ProfileForm::from_profile(&p);
        let again = back.to_profile().unwrap();
        assert_eq!(again.extra_env, p.extra_env);
        assert_eq!(again.prices, p.prices);
        assert_eq!(again.endpoint, p.endpoint);
        assert_eq!(
            kind_from_value(kind_value(ProfileKind::Custom)),
            ProfileKind::Custom
        );
    }
}
