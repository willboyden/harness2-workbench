//! Thin wrappers over browser APIs. Every call that can throw is used through its `Result`
//! binding, so a blocked `localStorage`, a missing clipboard or a refused download degrades
//! gracefully instead of aborting the app.

use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::Closure;
use web_sys::{Blob, BlobPropertyBag, HtmlAnchorElement, HtmlElement, Url};

/// Current time in Unix seconds.
pub(crate) fn now_secs() -> i64 {
    let ms = js_sys::Date::now();
    if !ms.is_finite() || ms < 0.0 {
        return 0;
    }
    // Milliseconds since 1970 fit easily in i64 seconds; clamped above to be non-negative.
    #[allow(clippy::cast_possible_truncation)]
    let secs = (ms / 1000.0).floor() as i64;
    secs
}

/// Read a `localStorage` key. `None` when storage is unavailable, blocked or empty.
pub(crate) fn storage_get(key: &str) -> Option<String> {
    let storage = window().local_storage().ok()??;
    storage.get_item(key).ok()?
}

/// Write a `localStorage` key; failures (private mode, quota, blocked) are ignored.
pub(crate) fn storage_set(key: &str, value: &str) {
    if let Ok(Some(storage)) = window().local_storage() {
        let _ = storage.set_item(key, value);
    }
}

/// Set `data-theme` on `<html>` (or remove it to follow the system preference).
pub(crate) fn apply_theme(value: Option<&str>) {
    if let Some(root) = document().document_element() {
        let _ = match value {
            Some(v) => root.set_attribute("data-theme", v),
            None => root.remove_attribute("data-theme"),
        };
    }
}

pub(crate) fn set_title(title: &str) {
    if title.is_empty() {
        document().set_title("Harness² Studio");
    } else {
        document().set_title(&format!("{title} · Harness² Studio"));
    }
}

/// Focus an element by id; `false` when it does not exist or cannot take focus.
pub(crate) fn focus_id(id: &str) -> bool {
    document()
        .get_element_by_id(id)
        .and_then(|e| e.dyn_into::<HtmlElement>().ok())
        .is_some_and(|e| e.focus().is_ok())
}

/// The element that has focus, to restore it after a dialog closes.
pub(crate) fn active_element() -> Option<HtmlElement> {
    document()
        .active_element()
        .and_then(|e| e.dyn_into::<HtmlElement>().ok())
}

/// True when the event target is a text field, select or editable region, where single-key
/// shortcuts must not fire.
pub(crate) fn typing_in_field(ev: &web_sys::KeyboardEvent) -> bool {
    let Some(el) = ev
        .target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
    else {
        return false;
    };
    let tag = el.tag_name().to_ascii_lowercase();
    matches!(tag.as_str(), "input" | "textarea" | "select")
        || el
            .get_attribute("contenteditable")
            .is_some_and(|v| v != "false")
}

/// Copy text to the clipboard, then call `done(true)` on success or `done(false)` when the
/// clipboard is unavailable (insecure context) or the browser refused.
pub(crate) fn copy_text(text: &str, done: impl Fn(bool) + Clone + 'static) {
    let navigator = window().navigator();
    let has_clipboard = js_sys::Reflect::get(&navigator, &"clipboard".into())
        .is_ok_and(|v| !v.is_undefined() && !v.is_null());
    if !has_clipboard {
        done(false);
        return;
    }
    let promise = navigator.clipboard().write_text(text);
    let ok_done = done.clone();
    let ok = Closure::<dyn FnMut(wasm_bindgen::JsValue)>::new(move |_| ok_done(true));
    let err = Closure::<dyn FnMut(wasm_bindgen::JsValue)>::new(move |_| done(false));
    let _ = promise.then2(&ok, &err);
    // The promise settles exactly once; the two small closures are released to JS.
    ok.forget();
    err.forget();
}

/// Offer `text` as a file download (`filename`, MIME type `mime`).
pub(crate) fn download_text(filename: &str, mime: &str, text: &str) -> bool {
    let parts = js_sys::Array::new();
    parts.push(&wasm_bindgen::JsValue::from_str(text));
    let opts = BlobPropertyBag::new();
    opts.set_type(mime);
    let Ok(blob) = Blob::new_with_str_sequence_and_options(&parts, &opts) else {
        return false;
    };
    let Ok(url) = Url::create_object_url_with_blob(&blob) else {
        return false;
    };
    let started = document()
        .create_element("a")
        .ok()
        .and_then(|e| e.dyn_into::<HtmlAnchorElement>().ok())
        .is_some_and(|a| {
            a.set_href(&url);
            a.set_download(filename);
            a.set_rel("noopener");
            let body = document().body();
            if let Some(b) = &body {
                let _ = b.append_child(&a);
            }
            a.click();
            if let Some(b) = &body {
                let _ = b.remove_child(&a);
            }
            true
        });
    // Revoke later: revoking at once can cancel the download in some browsers.
    set_timeout(
        move || {
            let _ = Url::revoke_object_url(&url);
        },
        std::time::Duration::from_secs(30),
    );
    started
}

/// Scroll an element to its bottom (live log auto-scroll).
pub(crate) fn scroll_to_bottom(id: &str) {
    if let Some(el) = document().get_element_by_id(id) {
        el.set_scroll_top(el.scroll_height());
    }
}

/// Whether an element is scrolled (within a few pixels) to its bottom.
pub(crate) fn is_at_bottom(id: &str) -> bool {
    document()
        .get_element_by_id(id)
        .is_none_or(|el| el.scroll_height() - el.scroll_top() - el.client_height() <= 8)
}
