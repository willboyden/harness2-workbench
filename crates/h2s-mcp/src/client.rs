//! Client for the Harness² Studio server's REST API (`/api/v1`, `docs/ARCHITECTURE.md` §8).
//!
//! Every request carries the caller's bearer token; every unsafe request (anything but
//! GET/HEAD/OPTIONS) also carries `x-h2s-request: 1`. Request and response bodies are the
//! `h2s_api` wire types. Non-2xx responses are parsed into the server's [`ErrorBody`].

use std::{net::IpAddr, sync::Arc, time::Duration};

use h2s_api::{API_PREFIX, CSRF_HEADER, ErrorBody};
use reqwest::{
    Method, StatusCode,
    header::{ACCEPT, CONTENT_TYPE},
};
use serde::{Serialize, de::DeserializeOwned};
use url::Url;

use crate::token::Token;

/// Largest response body read into memory (results with file contents can be big; a
/// misbehaving server must not exhaust memory).
const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
/// Largest error body read (they are one short JSON object).
const MAX_ERROR_BYTES: usize = 64 * 1024;
/// Longest path segment sent (identifiers are far shorter).
const MAX_SEGMENT_LEN: usize = 512;

/// How the client talks to the server.
#[derive(Debug, Clone)]
pub struct ClientOptions {
    /// Total time allowed for one ordinary request (not for event streams).
    pub request_timeout: Duration,
    /// Time allowed to establish a connection.
    pub connect_timeout: Duration,
    /// Allow `http://` to a non-loopback host (tokens would cross the network unencrypted).
    pub allow_insecure_remote: bool,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(120),
            connect_timeout: Duration::from_secs(10),
            allow_insecure_remote: false,
        }
    }
}

/// A configuration problem with the server URL or the HTTP client.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid server URL {url:?}: {reason}")]
    BadUrl { url: String, reason: String },
    #[error(
        "refusing to send API tokens over plain http to {host}: use an https URL (a TLS \
         reverse proxy in front of h2studio) or pass --allow-insecure-remote"
    )]
    InsecureRemote { host: String },
    #[error("cannot build the HTTP client: {0}")]
    Client(String),
}

/// Why an API call failed.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ApiError {
    /// The server answered with its JSON error body.
    #[error("{}", .body.error.message)]
    Api { status: u16, body: ErrorBody },
    /// The server (or something in front of it) answered with a non-JSON error.
    #[error("HTTP {status}: {snippet}")]
    Http { status: u16, snippet: String },
    /// The server could not be reached or did not answer in time.
    #[error("{0}")]
    Transport(String),
    /// A 2xx response did not match the contract type.
    #[error("unexpected response from the server: {0}")]
    Decode(String),
    /// The request could not be built from the given arguments.
    #[error("{0}")]
    Invalid(String),
}

impl ApiError {
    /// HTTP status of a server answer, if there was one.
    #[must_use]
    pub fn status(&self) -> Option<u16> {
        match self {
            ApiError::Api { status, .. } | ApiError::Http { status, .. } => Some(*status),
            _ => None,
        }
    }
}

/// The REST client. Cheap to clone; holds no per-caller state (tokens are passed per call).
#[derive(Clone)]
pub struct ApiClient {
    inner: Arc<Inner>,
}

struct Inner {
    http: reqwest::Client,
    /// Base URL with a trailing slash, e.g. `http://127.0.0.1:8722/`.
    base: Url,
    request_timeout: Duration,
}

impl std::fmt::Debug for ApiClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiClient")
            .field("base", &self.inner.base.as_str())
            .finish_non_exhaustive()
    }
}

impl ApiClient {
    /// Build a client for the server at `server_url` (`http(s)://host[:port][/base/path]`).
    ///
    /// Loopback servers are reached without any proxy; plain `http` to anything else is
    /// refused unless `allow_insecure_remote` is set.
    ///
    /// # Errors
    /// The URL is invalid or insecure, or the TLS backend cannot initialise.
    pub fn new(server_url: &str, options: &ClientOptions) -> Result<Self, ConfigError> {
        let bad = |reason: &str| ConfigError::BadUrl {
            url: server_url.to_owned(),
            reason: reason.to_owned(),
        };
        let mut base = Url::parse(server_url.trim()).map_err(|e| bad(&e.to_string()))?;
        if !matches!(base.scheme(), "http" | "https") {
            return Err(bad("the scheme must be http or https"));
        }
        if !base.username().is_empty() || base.password().is_some() {
            return Err(bad("credentials do not belong in the URL; use a token"));
        }
        if base.query().is_some() || base.fragment().is_some() {
            return Err(bad("the URL must not have a query or fragment"));
        }
        let host = base
            .host_str()
            .ok_or_else(|| bad("the URL has no host"))?
            .to_owned();
        let loopback = is_loopback_host(&host);
        if base.scheme() == "http" && !loopback && !options.allow_insecure_remote {
            return Err(ConfigError::InsecureRemote { host });
        }
        if !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }

        let mut builder = reqwest::Client::builder()
            .user_agent(concat!("h2studio-mcp/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(options.connect_timeout)
            // Never follow redirects: the API does not redirect, and following one could
            // carry the bearer token somewhere else.
            .redirect(reqwest::redirect::Policy::none());
        if loopback {
            // A system proxy must never see loopback traffic (or the token in it).
            builder = builder.no_proxy();
        }
        let http = builder
            .build()
            .map_err(|e| ConfigError::Client(e.to_string()))?;
        Ok(Self {
            inner: Arc::new(Inner {
                http,
                base,
                request_timeout: options.request_timeout,
            }),
        })
    }

    /// The server's base URL, for messages.
    #[must_use]
    pub fn server_url(&self) -> &str {
        self.inner.base.as_str()
    }

    /// `GET` an API route and decode its JSON body.
    pub(crate) async fn get<T: DeserializeOwned>(
        &self,
        token: &Token,
        segments: &[&str],
        query: &[(&str, String)],
    ) -> Result<T, ApiError> {
        let url = self.api_url(segments, query)?;
        let resp = self
            .send(token, Method::GET, url, None, "application/json")
            .await?;
        decode(resp).await
    }

    /// `GET` an API route that answers `text/plain`.
    pub(crate) async fn get_text(
        &self,
        token: &Token,
        segments: &[&str],
        query: &[(&str, String)],
    ) -> Result<String, ApiError> {
        let url = self.api_url(segments, query)?;
        let resp = self
            .send(token, Method::GET, url, None, "text/plain")
            .await?;
        let bytes = read_capped(resp, MAX_RESPONSE_BYTES).await?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// `POST` a JSON body to an API route and decode the JSON answer.
    pub(crate) async fn post<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        token: &Token,
        segments: &[&str],
        body: Option<&B>,
    ) -> Result<T, ApiError> {
        self.send_json(token, Method::POST, segments, body).await
    }

    /// `PATCH` a JSON body to an API route and decode the JSON answer.
    pub(crate) async fn patch<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        token: &Token,
        segments: &[&str],
        body: &B,
    ) -> Result<T, ApiError> {
        self.send_json(token, Method::PATCH, segments, Some(body))
            .await
    }

    async fn send_json<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        token: &Token,
        method: Method,
        segments: &[&str],
        body: Option<&B>,
    ) -> Result<T, ApiError> {
        let url = self.api_url(segments, &[])?;
        let body = body
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|e| ApiError::Invalid(format!("cannot encode the request: {e}")))?;
        let resp = self
            .send(token, method, url, body, "application/json")
            .await?;
        decode(resp).await
    }

    /// Open `GET /api/v1/jobs/{id}/events` (Server-Sent Events). The caller reads it with
    /// [`reqwest::Response::chunk`]; no total timeout applies to the stream.
    pub(crate) async fn job_events(
        &self,
        token: &Token,
        job_id: &str,
    ) -> Result<reqwest::Response, ApiError> {
        let url = self.api_url(&["jobs", job_id, "events"], &[])?;
        let resp = self
            .inner
            .http
            .get(url)
            .bearer_auth(token.secret())
            .header(ACCEPT, "text/event-stream")
            .send()
            .await
            .map_err(|e| self.transport_error(&e))?;
        if resp.status().is_success() {
            Ok(resp)
        } else {
            Err(error_from_response(resp).await)
        }
    }

    /// Build `<base>/api/v1/<segments...>?<query>`, percent-encoding every segment.
    fn api_url(&self, segments: &[&str], query: &[(&str, String)]) -> Result<Url, ApiError> {
        let mut url = self.inner.base.clone();
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|()| ApiError::Invalid("the server URL cannot take a path".into()))?;
            path.pop_if_empty();
            for prefix in API_PREFIX.split('/').filter(|s| !s.is_empty()) {
                path.push(prefix);
            }
            for segment in segments {
                check_segment(segment)?;
                path.push(segment);
            }
        }
        if !query.is_empty() {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in query {
                pairs.append_pair(key, value);
            }
        }
        Ok(url)
    }

    async fn send(
        &self,
        token: &Token,
        method: Method,
        url: Url,
        body: Option<Vec<u8>>,
        accept: &str,
    ) -> Result<reqwest::Response, ApiError> {
        let unsafe_method = !matches!(method, Method::GET | Method::HEAD | Method::OPTIONS);
        let mut request = self
            .inner
            .http
            .request(method, url)
            .bearer_auth(token.secret())
            .header(ACCEPT, accept)
            .timeout(self.inner.request_timeout);
        if unsafe_method {
            request = request.header(CSRF_HEADER, "1");
        }
        if let Some(body) = body {
            request = request.header(CONTENT_TYPE, "application/json").body(body);
        }
        let resp = request.send().await.map_err(|e| self.transport_error(&e))?;
        if resp.status().is_success() {
            Ok(resp)
        } else {
            Err(error_from_response(resp).await)
        }
    }

    fn transport_error(&self, err: &reqwest::Error) -> ApiError {
        let server = self.inner.base.as_str();
        let detail = if err.is_timeout() {
            format!(
                "the Harness² Studio server at {server} did not answer within {} s",
                self.inner.request_timeout.as_secs()
            )
        } else if err.is_connect() {
            format!(
                "cannot connect to the Harness² Studio server at {server}: is `h2studio serve` \
                 running, and is H2S_SERVER_URL / --server-url right?"
            )
        } else {
            format!("request to the Harness² Studio server at {server} failed: {err}")
        };
        ApiError::Transport(detail)
    }
}

/// Reject segments the URL library would silently drop or that cannot be an identifier.
fn check_segment(segment: &str) -> Result<(), ApiError> {
    let invalid = |why: &str| ApiError::Invalid(format!("invalid identifier {segment:?}: {why}"));
    if segment.is_empty() {
        return Err(invalid("it is empty"));
    }
    if segment == "." || segment == ".." {
        return Err(invalid("'.' and '..' are not identifiers"));
    }
    if segment.len() > MAX_SEGMENT_LEN {
        return Err(invalid("it is too long"));
    }
    if segment.chars().any(char::is_control) {
        return Err(invalid("it contains control characters"));
    }
    Ok(())
}

/// `localhost`, `*.localhost` or a loopback IP literal.
#[must_use]
pub fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") {
        return true;
    }
    host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

async fn decode<T: DeserializeOwned>(resp: reqwest::Response) -> Result<T, ApiError> {
    let bytes = read_capped(resp, MAX_RESPONSE_BYTES).await?;
    serde_json::from_slice(&bytes).map_err(|e| {
        ApiError::Decode(format!(
            "{e} (body starts with: {})",
            snippet(&String::from_utf8_lossy(&bytes))
        ))
    })
}

async fn error_from_response(resp: reqwest::Response) -> ApiError {
    let status = resp.status();
    let bytes = read_capped(resp, MAX_ERROR_BYTES).await.unwrap_or_default();
    match serde_json::from_slice::<ErrorBody>(&bytes) {
        Ok(body) => ApiError::Api {
            status: status.as_u16(),
            body,
        },
        Err(_) => {
            let text = String::from_utf8_lossy(&bytes);
            let shown = if text.trim().is_empty() {
                status
                    .canonical_reason()
                    .unwrap_or("no response body")
                    .to_owned()
            } else {
                snippet(&text)
            };
            ApiError::Http {
                status: status.as_u16(),
                snippet: shown,
            }
        }
    }
}

async fn read_capped(mut resp: reqwest::Response, cap: usize) -> Result<Vec<u8>, ApiError> {
    let mut buf = Vec::new();
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                if buf.len() + chunk.len() > cap {
                    return Err(ApiError::Decode(format!(
                        "the response is larger than {} MiB",
                        cap / (1024 * 1024)
                    )));
                }
                buf.extend_from_slice(&chunk);
            }
            Ok(None) => return Ok(buf),
            Err(e) => {
                return Err(ApiError::Transport(format!(
                    "reading the server's response failed: {e}"
                )));
            }
        }
    }
}

/// First 200 characters of a body, on one line.
fn snippet(text: &str) -> String {
    let one_line: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(200)
        .collect();
    one_line.trim().to_owned()
}

/// True for statuses worth retrying while waiting (server restarting, overloaded).
pub(crate) fn is_transient(err: &ApiError) -> bool {
    match err {
        ApiError::Transport(_) => true,
        ApiError::Api { status, .. } | ApiError::Http { status, .. } => matches!(
            StatusCode::from_u16(*status),
            Ok(StatusCode::BAD_GATEWAY
                | StatusCode::SERVICE_UNAVAILABLE
                | StatusCode::GATEWAY_TIMEOUT
                | StatusCode::TOO_MANY_REQUESTS)
        ),
        ApiError::Decode(_) | ApiError::Invalid(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(url: &str) -> ApiClient {
        ApiClient::new(url, &ClientOptions::default()).unwrap()
    }

    #[test]
    fn builds_api_urls_with_encoded_segments() {
        let c = client("http://127.0.0.1:8722");
        let url = c
            .api_url(
                &["experiments", "a/b", "sweeps", "x?y#z", "tasks", "t%2e"],
                &[("path", "dir/file name.txt".into())],
            )
            .unwrap();
        assert_eq!(
            url.as_str(),
            "http://127.0.0.1:8722/api/v1/experiments/a%2Fb/sweeps/x%3Fy%23z/tasks/t%252e?path=dir%2Ffile+name.txt"
        );
    }

    #[test]
    fn keeps_a_base_path() {
        let c = client("https://studio.example.com/h2s");
        let url = c.api_url(&["jobs"], &[]).unwrap();
        assert_eq!(url.as_str(), "https://studio.example.com/h2s/api/v1/jobs");
        let c = client("https://studio.example.com/h2s/");
        let url = c.api_url(&["jobs"], &[]).unwrap();
        assert_eq!(url.as_str(), "https://studio.example.com/h2s/api/v1/jobs");
    }

    #[test]
    fn rejects_dot_segments_instead_of_dropping_them() {
        let c = client("http://127.0.0.1:8722");
        for bad in ["", ".", "..", "a\nb"] {
            assert!(
                c.api_url(&["experiments", bad, "manifest"], &[]).is_err(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn validates_server_urls() {
        let opts = ClientOptions::default();
        for bad in [
            "ftp://127.0.0.1",
            "http://user:pw@127.0.0.1:8722",
            "http://127.0.0.1:8722/?x=1",
            "not a url",
        ] {
            assert!(ApiClient::new(bad, &opts).is_err(), "{bad}");
        }
        assert!(matches!(
            ApiClient::new("http://studio.example.com:8722", &opts),
            Err(ConfigError::InsecureRemote { .. })
        ));
        let lenient = ClientOptions {
            allow_insecure_remote: true,
            ..ClientOptions::default()
        };
        assert!(ApiClient::new("http://studio.example.com:8722", &lenient).is_ok());
        assert!(ApiClient::new("https://studio.example.com", &opts).is_ok());
        assert!(ApiClient::new("http://localhost:8722", &opts).is_ok());
        assert!(ApiClient::new("http://[::1]:8722", &opts).is_ok());
    }

    #[test]
    fn loopback_detection() {
        for host in [
            "localhost",
            "LOCALHOST",
            "a.localhost",
            "127.0.0.1",
            "127.9.9.9",
            "[::1]",
        ] {
            assert!(is_loopback_host(host), "{host}");
        }
        for host in [
            "example.com",
            "10.0.0.1",
            "0.0.0.0",
            "localhost.example.com",
        ] {
            assert!(!is_loopback_host(host), "{host}");
        }
    }

    #[test]
    fn transient_errors() {
        assert!(is_transient(&ApiError::Transport("x".into())));
        assert!(is_transient(&ApiError::Http {
            status: 503,
            snippet: String::new()
        }));
        assert!(!is_transient(&ApiError::Http {
            status: 404,
            snippet: String::new()
        }));
    }
}
