//! The web UI, embedded from `dist/web/` (built by `cargo xtask web`).
//!
//! Only an allowlist is served: `/` (`index.html`), `/app.js`, `/styles.css`,
//! `/favicon.svg` and `/pkg/*` (wasm-bindgen output). Any other GET outside `/api/` and the
//! root endpoints returns `index.html` so client-side routes work; nothing else on disk is
//! ever reachable. The server builds and runs without `dist/web` and then serves a small
//! built-in page saying how to build the UI. Hashed asset names are cached `immutable`;
//! everything else must be revalidated (`no-cache`, with an `ETag`).

use axum::body::Body;
use axum::extract::Request;
use axum::response::{IntoResponse, Response};
use http::{HeaderValue, Method, StatusCode, header};

use crate::error::ApiError;

#[derive(rust_embed::RustEmbed)]
#[folder = "../../dist/web"]
#[allow_missing = true]
#[include = "index.html"]
#[include = "app.js"]
#[include = "styles.css"]
#[include = "favicon.svg"]
#[include = "pkg/*"]
struct WebAssets;

/// Shown when the UI was not built into this binary. No inline script or style (CSP).
pub const NOT_BUILT_PAGE: &str = "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<title>Harness² Workbench</title>\n</head>\n<body>\n<h1>Harness² Workbench</h1>\n<p>UI not built — run <code>cargo xtask web</code>, then rebuild the server.</p>\n<p>The REST API is available under <code>/api/v1</code>.</p>\n</body>\n</html>\n";

const ROOT_FILES: &[&str] = &["index.html", "app.js", "styles.css", "favicon.svg"];

/// The embedded path for a request path, when it is on the allowlist.
#[must_use]
pub fn allowlisted(path: &str) -> Option<String> {
    let p = path.strip_prefix('/')?;
    if p.is_empty() {
        return Some("index.html".to_owned());
    }
    if ROOT_FILES.contains(&p) {
        return Some(p.to_owned());
    }
    let rest = p.strip_prefix("pkg/")?;
    let ok = !rest.is_empty()
        && rest.len() <= 200
        && rest.split('/').all(|seg| {
            !seg.is_empty()
                && !seg.starts_with('.')
                && seg
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        });
    ok.then(|| p.to_owned())
}

/// `name-<8+ hex>.ext` or `name.<8+ hex>.ext`: content-addressed, safe to cache forever.
#[must_use]
pub fn is_hashed(name: &str) -> bool {
    let file = name.rsplit('/').next().unwrap_or(name);
    let Some((stem, _)) = file.rsplit_once('.') else {
        return false;
    };
    let hash = stem
        .rsplit_once('-')
        .map(|(_, h)| h)
        .into_iter()
        .chain(stem.rsplit_once('.').map(|(_, h)| h))
        .find(|h| h.len() >= 8 && h.bytes().all(|b| b.is_ascii_hexdigit()));
    hash.is_some()
}

fn mime_for(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, e)| e) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("wasm") => "application/wasm",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn file_response(req: &Request, name: &str) -> Option<Response> {
    let file = WebAssets::get(name)?;
    let etag = format!("\"{}\"", hex::encode(&file.metadata.sha256_hash()[..16]));
    let cache = if is_hashed(name) {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let not_modified = req
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == etag));
    let mut resp = if not_modified {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        let mut r = Response::new(Body::from(file.data.into_owned()));
        r.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(mime_for(name)),
        );
        r
    };
    let h = resp.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    if let Ok(v) = HeaderValue::from_str(&etag) {
        h.insert(header::ETAG, v);
    }
    Some(resp)
}

fn index(req: &Request) -> Response {
    file_response(req, "index.html").unwrap_or_else(|| {
        let mut r = Response::new(Body::from(NOT_BUILT_PAGE));
        r.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        r.headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        r
    })
}

/// `true` when the UI is embedded in this build.
#[must_use]
pub fn ui_built() -> bool {
    WebAssets::get("index.html").is_some()
}

/// Router fallback: static files, the SPA fallback, and JSON 404s for the API.
pub async fn fallback(req: Request) -> Response {
    let path = req.uri().path().to_owned();
    let is_api = path == "/api" || path.starts_with("/api/");
    if is_api || !matches!(*req.method(), Method::GET | Method::HEAD) {
        return ApiError::not_found(format!("no route for {} {path}", req.method()))
            .into_response();
    }
    if let Some(name) = allowlisted(&path) {
        if name == "index.html" {
            return index(&req);
        }
        return file_response(&req, &name).unwrap_or_else(|| {
            ApiError::not_found(format!("{path} is not part of this build")).into_response()
        });
    }
    // Client-side routes (`/experiments/x`, `/jobs/y`, ...).
    index(&req)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn allowlist() {
        assert_eq!(allowlisted("/").as_deref(), Some("index.html"));
        assert_eq!(allowlisted("/app.js").as_deref(), Some("app.js"));
        assert_eq!(
            allowlisted("/pkg/h2w_web_bg.wasm").as_deref(),
            Some("pkg/h2w_web_bg.wasm")
        );
        assert_eq!(
            allowlisted("/pkg/snippets/x-1/inline0.js").as_deref(),
            Some("pkg/snippets/x-1/inline0.js")
        );
        for bad in [
            "/index.htm",
            "/pkg/",
            "/pkg/../secret",
            "/pkg/.hidden",
            "/pkg//x",
            "/Cargo.toml",
            "/pkg/a%2f..",
            "relative",
        ] {
            assert!(allowlisted(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn hashed_names() {
        assert!(is_hashed("pkg/h2w_web-1a2b3c4d.js"));
        assert!(is_hashed("app.0123456789abcdef.css"));
        assert!(!is_hashed("pkg/h2w_web_bg.wasm"));
        assert!(!is_hashed("app.js"));
        assert!(!is_hashed("index.html"));
        assert!(!is_hashed("x-zzzzzzzz.js"));
    }

    #[test]
    fn mime_types() {
        assert_eq!(mime_for("pkg/x.wasm"), "application/wasm");
        assert_eq!(mime_for("app.js"), "text/javascript; charset=utf-8");
        assert_eq!(mime_for("favicon.svg"), "image/svg+xml");
    }

    #[test]
    fn not_built_page_needs_no_inline_code() {
        assert!(!NOT_BUILT_PAGE.contains("<script"));
        assert!(!NOT_BUILT_PAGE.contains("style="));
        assert!(!NOT_BUILT_PAGE.contains("<style"));
    }

    proptest! {
        #[test]
        fn allowlisted_paths_never_escape(s in "[/a-z.%]{0,30}") {
            if let Some(p) = allowlisted(&s) {
                prop_assert!(p.split('/').all(|seg| !seg.is_empty() && !seg.starts_with('.')));
                prop_assert!(!p.starts_with('/'));
            }
        }
    }
}
