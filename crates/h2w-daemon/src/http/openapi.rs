//! The OpenAPI 3.1 document of the REST API, served at `/api/v1/openapi.json` and printed
//! by `h2workbench openapi`. No Swagger UI is bundled (it would load a CDN script).

use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

use super::routes::{admin, auth, experiments, jobs, results, root, system, tools};

struct Security;

impl Modify for Security {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("h2w_<id>_<secret>")
                    .description(Some("An API token"))
                    .build(),
            ),
        );
        components.add_security_scheme(
            "session",
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::with_description(
                h2w_api::SESSION_COOKIE,
                "Browser session from POST /api/v1/auth/login (named __Host-h2w_session when \
                 the server's public URL is https)",
            ))),
        );
        let reqs = vec![
            utoipa::openapi::security::SecurityRequirement::new("bearer", Vec::<String>::new()),
            utoipa::openapi::security::SecurityRequirement::new("session", Vec::<String>::new()),
        ];
        openapi.security = Some(reqs);
    }
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Harness² Workbench API",
        description = "REST API of Harness² Workbench, an unofficial GUI, API and MCP server for google-research/harness2. Every state-changing request needs the header `x-h2w-request: 1`.",
        license(name = "Apache-2.0", identifier = "Apache-2.0"),
    ),
    modifiers(&Security),
    paths(
        root::healthz, root::readyz, root::metrics,
        auth::login, auth::logout, auth::me,
        system::info, system::doctor, system::benchmarks, system::openapi,
        admin::list_profiles, admin::put_profile, admin::delete_profile, admin::check_profile,
        admin::list_secrets, admin::put_secret, admin::delete_secret,
        admin::list_tokens, admin::create_token, admin::revoke_token,
        admin::list_audit, admin::verify_audit,
        experiments::list, experiments::create, experiments::get_one, experiments::update,
        experiments::manifest,
        results::tasks, results::task, results::harness, results::diff, results::export,
        results::rollout_files, results::rollout_file, results::scores, results::report,
        results::library, results::audit_sessions, results::audit_text, results::compare,
        tools::estimate, tools::validate_harness, tools::splits, tools::choose_split,
        jobs::list, jobs::create, jobs::get_one, jobs::cancel, jobs::log, jobs::events,
    ),
    components(schemas(
        h2w_api::ErrorBody, h2w_api::ApiError, h2w_api::ErrorCode, h2w_api::FieldProblem,
        h2w_api::ExportTarget, results::ExportFormat, h2w_api::JobEvent, h2w_api::JobResult,
        h2w_api::PipelineResult,
    )),
    tags(
        (name = "system"), (name = "auth"), (name = "profiles"), (name = "secrets"),
        (name = "tokens"), (name = "audit"), (name = "experiments"), (name = "results"),
        (name = "tools"), (name = "jobs"),
    ),
)]
pub struct ApiDoc;

/// The document as JSON.
#[must_use]
pub fn document_json() -> serde_json::Value {
    let mut doc = ApiDoc::openapi();
    doc.info.version = env!("CARGO_PKG_VERSION").to_owned();
    serde_json::to_value(&doc).unwrap_or_else(|_| serde_json::json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_is_openapi_31_and_lists_every_route() {
        let doc = document_json();
        assert!(doc["openapi"].as_str().unwrap().starts_with("3.1"));
        let paths = doc["paths"].as_object().unwrap();
        for r in crate::http::ROUTES {
            let op = r.method.as_str().to_ascii_lowercase();
            let entry = paths
                .get(r.path)
                .unwrap_or_else(|| panic!("{} missing from the OpenAPI document", r.path));
            assert!(
                entry.get(&op).is_some(),
                "{} {} missing from the OpenAPI document",
                r.method,
                r.path
            );
        }
        let n_ops: usize = paths
            .values()
            .map(|v| v.as_object().map_or(0, |o| o.len()))
            .sum();
        assert_eq!(
            n_ops,
            crate::http::ROUTES.len(),
            "the document lists routes the router does not have"
        );
        assert!(doc["components"]["securitySchemes"]["bearer"].is_object());
        for name in [
            "Job",
            "JobResult",
            "PipelineResult",
            "JobEvent",
            "JobSpec",
            "PreviewResult",
        ] {
            assert!(
                doc["components"]["schemas"][name].is_object(),
                "schema {name} missing"
            );
        }
    }

    #[test]
    fn every_schema_reference_resolves() {
        let doc = document_json();
        let text = doc.to_string();
        let schemas = doc["components"]["schemas"].as_object().unwrap();
        let mut rest = text.as_str();
        while let Some(i) = rest.find("#/components/schemas/") {
            let tail = &rest[i + "#/components/schemas/".len()..];
            let name: String = tail.chars().take_while(|c| *c != '"').collect();
            assert!(schemas.contains_key(&name), "dangling $ref to {name}");
            rest = &tail[name.len()..];
        }
    }
}
