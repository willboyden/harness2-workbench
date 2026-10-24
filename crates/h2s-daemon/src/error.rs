//! The API error type: every non-2xx JSON response is an [`h2s_api::ErrorBody`].
//!
//! Handlers return [`ApiError`]; its response carries the body as an extension so the outer
//! middleware can add the request id (`http::guard`) before it is written.

use axum::response::{IntoResponse, Response};
use h2s_api::{ApiError as WireError, ErrorBody, ErrorCode, FieldProblem};
use http::{HeaderName, HeaderValue, StatusCode};

use crate::db::DbError;

/// An error response.
#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: ErrorCode,
    pub message: String,
    pub details: Vec<FieldProblem>,
    pub headers: Vec<(HeaderName, HeaderValue)>,
}

/// Marker extension: the response body is this error (filled with the request id later).
#[derive(Debug, Clone)]
pub struct PendingError(pub ErrorBody);

impl ApiError {
    #[must_use]
    pub fn new(status: StatusCode, code: ErrorCode, message: impl Into<String>) -> ApiError {
        ApiError {
            status,
            code,
            message: message.into(),
            details: Vec::new(),
            headers: Vec::new(),
        }
    }

    #[must_use]
    pub fn bad_request(m: impl Into<String>) -> ApiError {
        Self::new(StatusCode::BAD_REQUEST, ErrorCode::BadRequest, m)
    }

    /// 422 with one problem per field.
    #[must_use]
    pub fn validation(details: Vec<FieldProblem>) -> ApiError {
        let message = match details.as_slice() {
            [one] => format!("{}: {}", one.field, one.problem),
            _ => format!("{} problems with the request", details.len()),
        };
        ApiError {
            details,
            ..Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorCode::Validation,
                message,
            )
        }
    }

    /// 422 for one field.
    #[must_use]
    pub fn invalid(field: impl Into<String>, problem: impl Into<String>) -> ApiError {
        Self::validation(vec![FieldProblem {
            field: field.into(),
            problem: problem.into(),
        }])
    }

    #[must_use]
    pub fn unauthorized(m: impl Into<String>) -> ApiError {
        Self::new(StatusCode::UNAUTHORIZED, ErrorCode::Unauthorized, m)
    }

    #[must_use]
    pub fn forbidden(m: impl Into<String>) -> ApiError {
        Self::new(StatusCode::FORBIDDEN, ErrorCode::Forbidden, m)
    }

    #[must_use]
    pub fn not_found(m: impl Into<String>) -> ApiError {
        Self::new(StatusCode::NOT_FOUND, ErrorCode::NotFound, m)
    }

    #[must_use]
    pub fn conflict(m: impl Into<String>) -> ApiError {
        Self::new(StatusCode::CONFLICT, ErrorCode::Conflict, m)
    }

    #[must_use]
    pub fn too_large(limit: u64) -> ApiError {
        Self::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            ErrorCode::PayloadTooLarge,
            format!("the request body is larger than {limit} bytes"),
        )
    }

    #[must_use]
    pub fn rate_limited(retry_after_secs: u64) -> ApiError {
        let mut e = Self::new(
            StatusCode::TOO_MANY_REQUESTS,
            ErrorCode::RateLimited,
            format!("too many attempts; try again in {retry_after_secs} s"),
        );
        if let Ok(v) = HeaderValue::from_str(&retry_after_secs.to_string()) {
            e.headers.push((http::header::RETRY_AFTER, v));
        }
        e
    }

    #[must_use]
    pub fn misdirected() -> ApiError {
        Self::new(
            StatusCode::MISDIRECTED_REQUEST,
            ErrorCode::MisdirectedRequest,
            "this server does not answer to that Host",
        )
    }

    #[must_use]
    pub fn unavailable(m: impl Into<String>) -> ApiError {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, ErrorCode::Unavailable, m)
    }

    /// 500 with a safe message; log the cause separately.
    #[must_use]
    pub fn internal(m: impl Into<String>) -> ApiError {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, ErrorCode::Internal, m)
    }

    /// The `ErrorCode` for a status (used for errors no handler produced, e.g. 405).
    #[must_use]
    pub fn from_status(status: StatusCode) -> ApiError {
        let (code, msg) = match status.as_u16() {
            400 => (ErrorCode::BadRequest, "bad request"),
            401 => (ErrorCode::Unauthorized, "authentication required"),
            403 => (ErrorCode::Forbidden, "forbidden"),
            404 => (ErrorCode::NotFound, "not found"),
            405 => (ErrorCode::BadRequest, "method not allowed"),
            409 => (ErrorCode::Conflict, "conflict"),
            413 => (ErrorCode::PayloadTooLarge, "request body too large"),
            421 => (ErrorCode::MisdirectedRequest, "misdirected request"),
            422 => (ErrorCode::Validation, "invalid request"),
            429 => (ErrorCode::RateLimited, "too many requests"),
            503 | 504 | 408 => (ErrorCode::Unavailable, "service unavailable"),
            s if (400..500).contains(&s) => (ErrorCode::BadRequest, "bad request"),
            _ => (ErrorCode::Internal, "internal error"),
        };
        Self::new(status, code, msg)
    }

    /// The wire body.
    #[must_use]
    pub fn body(&self) -> ErrorBody {
        ErrorBody {
            error: WireError {
                code: self.code,
                message: self.message.clone(),
                details: self.details.clone(),
                request_id: None,
            },
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.status.as_u16(), self.message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = self.body();
        let json = serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec());
        let mut resp = (
            self.status,
            [(
                http::header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )],
            json,
        )
            .into_response();
        for (k, v) in self.headers {
            resp.headers_mut().insert(k, v);
        }
        resp.extensions_mut().insert(PendingError(body));
        resp
    }
}

impl From<DbError> for ApiError {
    fn from(e: DbError) -> ApiError {
        tracing::error!(error = %e, "database error");
        match e {
            DbError::Closed => ApiError::unavailable("the database is not available"),
            _ => ApiError::internal("internal database error"),
        }
    }
}

impl From<h2s_core::CoreError> for ApiError {
    fn from(e: h2s_core::CoreError) -> ApiError {
        use h2s_core::CoreError as C;
        match e {
            C::Invalid { field, problem } => ApiError::invalid(field, problem),
            C::InvalidMany(list) => ApiError::validation(
                list.into_iter()
                    .map(|(field, problem)| FieldProblem { field, problem })
                    .collect(),
            ),
            C::NotFound(what) => ApiError::not_found(format!("not found: {what}")),
            C::Refused(why) => ApiError::bad_request(format!("refused: {why}")),
            C::Malformed { what, detail } => {
                tracing::warn!(%what, %detail, "malformed harness2 output");
                ApiError::internal(format!("harness2 wrote a malformed {what}"))
            }
            C::Io(io) if io.kind() == std::io::ErrorKind::NotFound => {
                ApiError::not_found("not found")
            }
            C::Io(io) => {
                tracing::error!(error = %io, "I/O error reading results");
                ApiError::internal("I/O error reading results")
            }
            C::TooLarge(what) => ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                ErrorCode::PayloadTooLarge,
                format!("too large: {what}"),
            ),
            C::Unimplemented(what) => {
                ApiError::unavailable(format!("not available in this build yet: {what}"))
            }
        }
    }
}

impl From<tokio::task::JoinError> for ApiError {
    fn from(e: tokio::task::JoinError) -> ApiError {
        tracing::error!(error = %e, "background task failed");
        ApiError::internal("internal error")
    }
}

/// Remove quoted fragments (which may echo input values) from a parser message.
#[must_use]
pub fn scrub_quoted(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len());
    let mut in_quote = false;
    for c in msg.chars() {
        if c == '"' {
            if in_quote {
                out.push_str("…\"");
            } else {
                out.push('"');
            }
            in_quote = !in_quote;
        } else if !in_quote {
            out.push(c);
        }
    }
    if in_quote {
        out.push_str("…\"");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_removes_quoted_values() {
        assert_eq!(
            scrub_quoted("invalid type: string \"s3cret\", expected u32 at line 1"),
            "invalid type: string \"…\", expected u32 at line 1"
        );
        assert_eq!(scrub_quoted("missing field `name`"), "missing field `name`");
        assert_eq!(scrub_quoted("open \"abc"), "open \"…\"");
    }

    #[test]
    fn responses_carry_the_marker_and_headers() {
        let r = ApiError::rate_limited(7).into_response();
        assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(r.headers()[http::header::RETRY_AFTER], "7");
        let p = r.extensions().get::<PendingError>().unwrap();
        assert_eq!(p.0.error.code, ErrorCode::RateLimited);
    }

    #[test]
    fn validation_message_names_the_field() {
        let e = ApiError::invalid("k", "must be between 1 and 32");
        assert_eq!(e.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(e.message, "k: must be between 1 and 32");
    }

    #[test]
    fn core_errors_map_to_statuses() {
        let e: ApiError = h2s_core::CoreError::NotFound("sweep".into()).into();
        assert_eq!(e.status, StatusCode::NOT_FOUND);
        let e: ApiError = h2s_core::CoreError::Unimplemented("x").into();
        assert_eq!(e.status, StatusCode::SERVICE_UNAVAILABLE);
        let e: ApiError =
            h2s_core::CoreError::TooLarge("judged.json has more than 9 rows".into()).into();
        assert_eq!(
            (e.status, e.code, e.message.as_str()),
            (
                StatusCode::PAYLOAD_TOO_LARGE,
                ErrorCode::PayloadTooLarge,
                "too large: judged.json has more than 9 rows"
            )
        );
        let e: ApiError = h2s_core::CoreError::InvalidMany(vec![
            ("a".into(), "b".into()),
            ("c".into(), "d".into()),
        ])
        .into();
        assert_eq!(e.details.len(), 2);
    }
}
