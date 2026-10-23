//! Logging and tracing: `tracing` to stderr (pretty or JSON lines), and with the `otel`
//! feature and `telemetry.otlp_endpoint` set, span export over OTLP/HTTP (protobuf).
//! Request spans carry method, matched route, status and request id — never headers or
//! bodies. Standard output stays free for machine-readable command output.

use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::config::{Config, LogFormat};

/// Keeps the trace exporter alive; flushes it on drop.
#[derive(Default)]
pub struct TelemetryGuard {
    #[cfg(feature = "otel")]
    provider: Option<opentelemetry_sdk::trace::SdkTracerProvider>,
}

impl std::fmt::Debug for TelemetryGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TelemetryGuard")
    }
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        #[cfg(feature = "otel")]
        if let Some(p) = self.provider.take() {
            let _ = p.shutdown();
        }
    }
}

/// Install the global subscriber.
///
/// # Errors
/// An invalid filter, an exporter that cannot be built, or a subscriber already installed.
pub fn init(cfg: &Config) -> Result<TelemetryGuard, String> {
    let filter = EnvFilter::try_new(&cfg.file.telemetry.log_level).map_err(|e| e.to_string())?;
    let json = cfg.file.telemetry.log_format == LogFormat::Json;
    let ansi = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let pretty_layer = (!json).then(|| {
        tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_ansi(ansi)
            .with_target(false)
    });
    let json_layer = json.then(|| {
        tracing_subscriber::fmt::layer()
            .json()
            .with_writer(std::io::stderr)
            .with_current_span(true)
    });
    #[allow(unused_mut)]
    let mut guard = TelemetryGuard::default();
    #[cfg(feature = "otel")]
    let otel_layer = match &cfg.otlp_traces_url {
        Some(url) => {
            use opentelemetry::trace::TracerProvider as _;
            use opentelemetry_otlp::WithExportConfig;
            let exporter = opentelemetry_otlp::SpanExporter::builder()
                .with_http()
                .with_endpoint(url.clone())
                .build()
                .map_err(|e| format!("OTLP exporter: {e}"))?;
            let resource = opentelemetry_sdk::Resource::builder()
                .with_service_name(cfg.file.telemetry.service_name.clone())
                .build();
            let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
                .with_batch_exporter(exporter)
                .with_resource(resource)
                .build();
            let tracer = provider.tracer("h2studio");
            guard.provider = Some(provider);
            Some(tracing_opentelemetry::layer().with_tracer(tracer))
        }
        None => None,
    };
    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(pretty_layer)
        .with(json_layer);
    #[cfg(feature = "otel")]
    let registry = registry.with(otel_layer);
    registry.try_init().map_err(|e| e.to_string())?;
    Ok(guard)
}

/// Warnings about the environment worth logging at startup (odd XDG variables set by
/// sandboxed desktop apps, for example).
#[must_use]
pub fn environment_warnings(env: crate::config::EnvLookup<'_>) -> Vec<String> {
    let mut out = Vec::new();
    for name in ["XDG_CONFIG_HOME", "XDG_DATA_HOME"] {
        if let Some(v) = env(name) {
            if !v.starts_with('/') {
                out.push(format!("{name} is not absolute and is ignored"));
            } else if v.contains("/snap/") || v.contains("/.var/app/") {
                out.push(format!(
                    "{name} points into a sandboxed application's directory; set --config or H2S_DATA_DIR if that is not intended"
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn odd_xdg_values_are_flagged() {
        let env = |n: &str| match n {
            "XDG_CONFIG_HOME" => Some("relative".to_owned()),
            "XDG_DATA_HOME" => Some("/var/lib/u/snap/code/123/.local/share".to_owned()),
            _ => None,
        };
        let w = environment_warnings(&env);
        assert_eq!(w.len(), 2);
        assert!(environment_warnings(&|_| None).is_empty());
    }
}
