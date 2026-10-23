//! Turning API failures into messages an agent can act on.
//!
//! Every message starts with the server's error code word (`forbidden`, `validation`,
//! `unavailable`, ...), followed by the server's own message, field problems, and a short hint
//! where the fix is known.

use h2s_api::{ErrorCode, Role};

use crate::client::ApiError;

/// Render `err` for a tool that needs at least `needs`.
pub(crate) fn render(err: &ApiError, needs: Role) -> String {
    match err {
        ApiError::Api { body, .. } => {
            let e = &body.error;
            let mut msg = format!("{}: {}", code_word(e.code), e.message);
            for d in &e.details {
                msg.push_str(&format!("; {}: {}", d.field, d.problem));
            }
            match e.code {
                ErrorCode::Forbidden => msg.push_str(&format!(
                    " (this tool needs the {} role or higher; ask an admin for a token with that \
                     role)",
                    needs.as_str()
                )),
                ErrorCode::Unauthorized => msg.push_str(
                    " (the server rejected the API token: for stdio check H2S_TOKEN or \
                     H2S_TOKEN_FILE, over HTTP the bearer token your MCP client sends)",
                ),
                ErrorCode::Unavailable if mentions_missing_harness2(&e.message) => msg.push_str(
                    " — install it with install_harness2 (admin role), then wait_for_job",
                ),
                ErrorCode::RateLimited => msg.push_str(" (retry a little later)"),
                _ => {}
            }
            if let Some(id) = &e.request_id {
                msg.push_str(&format!(" [request id {id}]"));
            }
            msg
        }
        ApiError::Http { status, snippet } => {
            format!(
                "{}: HTTP {status} from the server: {snippet}",
                status_word(*status)
            )
        }
        ApiError::Transport(detail) => format!("unavailable: {detail}"),
        ApiError::Decode(detail) => format!("internal: {detail}"),
        ApiError::Invalid(detail) => format!("bad_request: {detail}"),
    }
}

/// The contract's snake_case name of an error code (`forbidden`, `not_found`, ...).
pub(crate) fn code_word(code: ErrorCode) -> String {
    match serde_json::to_value(code) {
        Ok(serde_json::Value::String(word)) => word,
        _ => format!("{code:?}").to_ascii_lowercase(),
    }
}

/// Error code word for a bare HTTP status (a proxy in front of the server, for example).
fn status_word(status: u16) -> &'static str {
    match status {
        400 => "bad_request",
        401 => "unauthorized",
        403 => "forbidden",
        404 => "not_found",
        409 => "conflict",
        413 => "payload_too_large",
        421 => "misdirected_request",
        422 => "validation",
        429 => "rate_limited",
        502..=504 => "unavailable",
        _ => "internal",
    }
}

fn mentions_missing_harness2(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("harness2") && lower.contains("not installed")
}

#[cfg(test)]
mod tests {
    use h2s_api::{ApiError as WireError, ErrorBody, FieldProblem};

    use super::*;

    fn api(code: ErrorCode, message: &str, details: Vec<FieldProblem>) -> ApiError {
        ApiError::Api {
            status: 400,
            body: ErrorBody {
                error: WireError {
                    code,
                    message: message.into(),
                    details,
                    request_id: Some("r-1".into()),
                },
            },
        }
    }

    #[test]
    fn forbidden_names_the_role() {
        let msg = render(
            &api(ErrorCode::Forbidden, "operator role required", vec![]),
            Role::Operator,
        );
        assert!(
            msg.starts_with("forbidden: operator role required"),
            "{msg}"
        );
        assert!(msg.contains("needs the operator role"), "{msg}");
        assert!(msg.ends_with("[request id r-1]"), "{msg}");
    }

    #[test]
    fn validation_lists_fields() {
        let msg = render(
            &api(
                ErrorCode::Validation,
                "invalid job spec",
                vec![FieldProblem {
                    field: "k".into(),
                    problem: "must be between 1 and 32".into(),
                }],
            ),
            Role::Operator,
        );
        assert!(
            msg.starts_with("validation: invalid job spec; k: must be between 1 and 32"),
            "{msg}"
        );
    }

    #[test]
    fn missing_harness2_points_at_install() {
        let msg = render(
            &api(ErrorCode::Unavailable, "harness2 is not installed", vec![]),
            Role::Operator,
        );
        assert!(msg.starts_with("unavailable: harness2 is not installed"));
        assert!(msg.contains("install_harness2 (admin role)"), "{msg}");
    }

    #[test]
    fn transport_and_http_errors_have_code_words() {
        assert!(
            render(&ApiError::Transport("down".into()), Role::Viewer).starts_with("unavailable: ")
        );
        let msg = render(
            &ApiError::Http {
                status: 502,
                snippet: "Bad Gateway".into(),
            },
            Role::Viewer,
        );
        assert_eq!(msg, "unavailable: HTTP 502 from the server: Bad Gateway");
        assert_eq!(code_word(ErrorCode::NotFound), "not_found");
        assert_eq!(
            code_word(ErrorCode::MisdirectedRequest),
            "misdirected_request"
        );
    }
}
