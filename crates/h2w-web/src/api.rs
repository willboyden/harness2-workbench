//! The REST client: same-origin `fetch` carrying the HttpOnly session cookie. Every unsafe
//! request sends `x-h2w-request: 1` (the server's CSRF guard); every non-2xx response becomes
//! an [`ApiError`] built from the server's `ErrorBody`; a 401 sends the user to the login page.

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use gloo_net::http::{Request, RequestBuilder, Response};
use h2w_api::{CSRF_HEADER, ErrorBody, ErrorCode, FieldProblem};
use serde::Serialize;
use serde::de::DeserializeOwned;
use web_sys::RequestCredentials;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ApiError {
    /// HTTP status; 0 when the server could not be reached.
    pub(crate) status: u16,
    pub(crate) code: Option<ErrorCode>,
    /// Safe to show: the server's `error.message`, or a sentence describing the failure.
    pub(crate) message: String,
    pub(crate) details: Vec<FieldProblem>,
    pub(crate) request_id: Option<String>,
}

impl ApiError {
    fn network(detail: &str) -> ApiError {
        ApiError {
            status: 0,
            code: None,
            message: format!("Could not reach the server ({detail}). Check that it is running."),
            details: Vec::new(),
            request_id: None,
        }
    }

    fn unreadable(status: u16, detail: &str) -> ApiError {
        ApiError {
            status,
            code: None,
            message: format!(
                "The server's answer could not be read by this version of the UI ({detail})."
            ),
            details: Vec::new(),
            request_id: None,
        }
    }

    pub(crate) fn is_unauthorized(&self) -> bool {
        self.status == 401
    }

    pub(crate) fn is_not_found(&self) -> bool {
        self.status == 404
    }

    /// The validation problem reported for one field, if any.
    pub(crate) fn field(&self, field: &str) -> Option<String> {
        self.details
            .iter()
            .find(|d| d.field == field)
            .map(|d| d.problem.clone())
    }

    /// A short heading for error panels.
    pub(crate) fn headline(&self) -> &'static str {
        match self.status {
            0 => "Server unreachable",
            401 => "Signed out",
            403 => "Not allowed",
            404 => "Not found",
            409 => "Conflict",
            413 => "Too large",
            422 => "Please check the input",
            429 => "Too many requests",
            503 => "Not available yet",
            s if s >= 500 => "Server error",
            _ => "Request failed",
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)?;
        for d in &self.details {
            write!(f, " {}: {}.", d.field, d.problem)?;
        }
        Ok(())
    }
}

thread_local! {
    static ON_UNAUTHORIZED: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) };
}

/// Register what happens when any request answers 401 (the app navigates to the login page).
pub(crate) fn on_unauthorized(f: impl Fn() + 'static) {
    ON_UNAUTHORIZED.with(|c| *c.borrow_mut() = Some(Rc::new(f)));
}

fn notify_unauthorized() {
    let handler = ON_UNAUTHORIZED.with(|c| c.borrow().clone());
    if let Some(h) = handler {
        h();
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

async fn send(method: Method, url: &str, json: Option<String>) -> Result<Response, ApiError> {
    let builder: RequestBuilder = match method {
        Method::Get => Request::get(url),
        Method::Post => Request::post(url),
        Method::Put => Request::put(url),
        Method::Patch => Request::patch(url),
        Method::Delete => Request::delete(url),
    }
    .credentials(RequestCredentials::SameOrigin)
    .header("accept", "application/json");
    let builder = if method == Method::Get {
        builder
    } else {
        builder.header(CSRF_HEADER, "1")
    };
    let request = match json {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(body),
        None => builder.build(),
    }
    .map_err(|e| ApiError::network(&e.to_string()))?;
    let resp = request
        .send()
        .await
        .map_err(|e| ApiError::network(&e.to_string()))?;
    if resp.ok() {
        return Ok(resp);
    }
    let status = resp.status();
    let err = match resp.json::<ErrorBody>().await {
        Ok(body) => ApiError {
            status,
            code: Some(body.error.code),
            message: body.error.message,
            details: body.error.details,
            request_id: body.error.request_id,
        },
        Err(_) => ApiError {
            status,
            code: None,
            message: format!("The server answered {status} {}.", resp.status_text()),
            details: Vec::new(),
            request_id: None,
        },
    };
    if status == 401 && !url.ends_with("/auth/login") && !url.ends_with("/auth/me") {
        notify_unauthorized();
    }
    Err(err)
}

async fn decode<T: DeserializeOwned>(resp: Response) -> Result<T, ApiError> {
    let status = resp.status();
    resp.json::<T>()
        .await
        .map_err(|e| ApiError::unreadable(status, &e.to_string()))
}

fn encode<B: Serialize + ?Sized>(body: &B) -> Result<String, ApiError> {
    serde_json::to_string(body).map_err(|e| ApiError {
        status: 0,
        code: None,
        message: format!("Could not encode the request: {e}"),
        details: Vec::new(),
        request_id: None,
    })
}

pub(crate) async fn get<T: DeserializeOwned>(url: &str) -> Result<T, ApiError> {
    decode(send(Method::Get, url, None).await?).await
}

/// `GET` a `text/plain` resource (audit session parts).
pub(crate) async fn get_text(url: &str) -> Result<String, ApiError> {
    let resp = send(Method::Get, url, None).await?;
    let status = resp.status();
    resp.text()
        .await
        .map_err(|e| ApiError::unreadable(status, &e.to_string()))
}

pub(crate) async fn post<B: Serialize + ?Sized, T: DeserializeOwned>(
    url: &str,
    body: &B,
) -> Result<T, ApiError> {
    decode(send(Method::Post, url, Some(encode(body)?)).await?).await
}

/// `POST` without a body that returns JSON (e.g. cancelling a job).
pub(crate) async fn post_empty<T: DeserializeOwned>(url: &str) -> Result<T, ApiError> {
    decode(send(Method::Post, url, None).await?).await
}

/// `POST` without a body whose answer has no content (logout).
pub(crate) async fn post_no_content(url: &str) -> Result<(), ApiError> {
    send(Method::Post, url, None).await.map(|_| ())
}

pub(crate) async fn put<B: Serialize + ?Sized, T: DeserializeOwned>(
    url: &str,
    body: &B,
) -> Result<T, ApiError> {
    decode(send(Method::Put, url, Some(encode(body)?)).await?).await
}

pub(crate) async fn patch<B: Serialize + ?Sized, T: DeserializeOwned>(
    url: &str,
    body: &B,
) -> Result<T, ApiError> {
    decode(send(Method::Patch, url, Some(encode(body)?)).await?).await
}

pub(crate) async fn delete(url: &str) -> Result<(), ApiError> {
    send(Method::Delete, url, None).await.map(|_| ())
}
