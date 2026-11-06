//! The (i) info button and its pop-up: the help of one form field or configuration key.
//!
//! The button says whether the pop-up is open (`aria-expanded`) and which element it opens
//! (`aria-controls`). The pop-up opens below the label row, in the page flow (so it never
//! covers the control or gets clipped by a scrolling container), and closes on Escape (focus
//! returns to the button), on a click outside it, when focus leaves both, or with its close
//! button. The control's `aria-describedby` points at the pop-up, so screen readers announce
//! the help whether or not it is open. Everything inside is text: nothing is parsed as HTML.

use leptos::ev;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use super::icons::{Icon, icon};
use crate::browser;

/// The state one info button and its pop-up share.
#[derive(Clone, Copy)]
pub(crate) struct Info {
    pub(crate) open: RwSignal<bool>,
    button_id: StoredValue<String>,
    pop_id: StoredValue<String>,
}

impl Info {
    /// `base` is the control's id; the pop-up is `<base>-help`, the button `<base>-info`.
    pub(crate) fn new(base: &str) -> Info {
        let info = Info {
            open: RwSignal::new(false),
            button_id: StoredValue::new(format!("{base}-info")),
            pop_id: StoredValue::new(format!("{base}-help")),
        };
        info.close_from_outside();
        info
    }

    pub(crate) fn pop_id(&self) -> String {
        self.pop_id.get_value()
    }

    fn close(&self, refocus: bool) {
        self.open.set(false);
        if refocus {
            let id = self.button_id.get_value();
            request_animation_frame(move || {
                browser::focus_id(&id);
            });
        }
    }

    /// While open, a press anywhere outside the button and the pop-up closes it.
    fn close_from_outside(self) {
        let handle = StoredValue::new(None::<WindowListenerHandle>);
        Effect::new(move |_| {
            if self.open.get() {
                let ids = (self.button_id.get_value(), self.pop_id.get_value());
                let h = window_event_listener(ev::mousedown, move |e| {
                    let inside = e
                        .target()
                        .and_then(|t| t.dyn_into::<web_sys::Node>().ok())
                        .is_some_and(|node| {
                            contains_node(&ids.0, &node) || contains_node(&ids.1, &node)
                        });
                    if !inside {
                        self.open.set(false);
                    }
                });
                handle.set_value(Some(h));
            } else if let Some(h) = handle.try_update_value(Option::take).flatten() {
                h.remove();
            }
        });
        on_cleanup(move || {
            if let Some(h) = handle.try_update_value(Option::take).flatten() {
                h.remove();
            }
        });
    }

    /// Escape closes the pop-up and returns focus to the button; it does not reach a dialog
    /// around the field (that would close the whole dialog).
    fn on_key(self, e: &web_sys::KeyboardEvent) {
        if e.key() == "Escape" && self.open.get_untracked() {
            e.prevent_default();
            e.stop_propagation();
            self.close(true);
        }
    }

    /// Focus moved somewhere that is neither the button nor the pop-up: close.
    fn on_focusout(self, e: &web_sys::FocusEvent) {
        let to = e
            .related_target()
            .and_then(|t| t.dyn_into::<web_sys::Node>().ok());
        let stays = to.as_ref().is_some_and(|node| {
            contains_node(&self.button_id.get_value(), node)
                || contains_node(&self.pop_id.get_value(), node)
        });
        if !stays && to.is_some() {
            self.open.set(false);
        }
    }
}

fn contains_node(id: &str, node: &web_sys::Node) -> bool {
    document()
        .get_element_by_id(id)
        .is_some_and(|el| el.contains(Some(node)))
}

/// The (i) button. `label` names what it explains ("About Judge model").
#[component]
pub(crate) fn InfoButton(info: Info, #[prop(into)] label: String) -> impl IntoView {
    view! {
        <button
            type="button"
            class="info-btn"
            id=info.button_id.get_value()
            aria-label=format!("About {label}")
            title=format!("About {label}")
            aria-expanded=move || if info.open.get() { "true" } else { "false" }
            aria-controls=info.pop_id()
            data-testid="info-button"
            on:click=move |_| info.open.update(|o| *o = !*o)
            on:keydown=move |e| info.on_key(&e)
            on:focusout=move |e| info.on_focusout(&e)
        >
            {icon(Icon::Info)}
        </button>
    }
}

/// The pop-up. Its children are rendered whether or not it is open (hidden while closed), so
/// `aria-describedby` on the control always has its text.
#[component]
pub(crate) fn InfoPop(
    info: Info,
    #[prop(into)] title: String,
    #[prop(optional)] wide: bool,
    children: Children,
) -> impl IntoView {
    let title_id = format!("{}-title", info.pop_id());
    let labelled_by = title_id.clone();
    view! {
        <div
            class=if wide { "info-pop info-pop-wide" } else { "info-pop" }
            id=info.pop_id()
            role="group"
            aria-labelledby=labelled_by
            tabindex="-1"
            hidden=move || !info.open.get()
            data-testid="info-pop"
            on:keydown=move |e| info.on_key(&e)
            on:focusout=move |e| info.on_focusout(&e)
        >
            <div class="info-pop-head">
                <p class="info-pop-title" id=title_id>{title}</p>
                <button
                    type="button"
                    class="icon-btn icon-btn-sm"
                    aria-label="Close"
                    on:click=move |_| info.close(true)
                >
                    {icon(Icon::Close)}
                </button>
            </div>
            <div class="info-pop-body">{children()}</div>
        </div>
    }
}
