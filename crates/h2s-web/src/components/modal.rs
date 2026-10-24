//! Modal dialogs. A dialog renders through a portal at the end of `<body>`, the app shell
//! behind it becomes `inert` (so focus and screen readers stay inside), Escape and the close
//! button dismiss it, and focus returns to where it was when it closes.

use leptos::portal::Portal;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use super::icons::{Icon, icon};
use super::ui::uid;
use crate::browser;
use crate::state::ui;

const FOCUSABLE: &str = "input:not([type=hidden]):not([disabled]), select:not([disabled]), textarea:not([disabled]), button:not([disabled]):not([data-close]), a[href], [tabindex]:not([tabindex='-1'])";

/// Focus the first focusable element in the dialog, or the dialog itself.
fn focus_first(dialog_id: &str) {
    let Some(dialog) = document().get_element_by_id(dialog_id) else {
        return;
    };
    // `querySelector` returns the first match in document order, so look for an explicit
    // `data-autofocus` first, then for anything focusable.
    let target = dialog
        .query_selector("[data-autofocus]")
        .ok()
        .flatten()
        .or_else(|| dialog.query_selector(FOCUSABLE).ok().flatten())
        .unwrap_or(dialog);
    if let Ok(el) = target.dyn_into::<web_sys::HtmlElement>() {
        let _ = el.focus();
    }
}

#[component]
pub(crate) fn Modal(
    #[prop(into)] open: Signal<bool>,
    on_close: Callback<()>,
    #[prop(into)] title: Signal<String>,
    #[prop(optional)] wide: bool,
    #[prop(optional)] testid: Option<&'static str>,
    children: ChildrenFn,
) -> impl IntoView {
    let u = ui();
    let dialog_id = uid("dlg");
    let title_id = format!("{dialog_id}-title");
    let restore = StoredValue::new_local(None::<web_sys::HtmlElement>);
    let counted = StoredValue::new(false);
    let children = StoredValue::new(children);
    {
        let dialog_id = dialog_id.clone();
        Effect::new(move |_| {
            let is_open = open.get();
            if is_open && !counted.get_value() {
                counted.set_value(true);
                restore.set_value(browser::active_element());
                u.modals.update(|n| *n += 1);
                let id = dialog_id.clone();
                request_animation_frame(move || focus_first(&id));
            } else if !is_open && counted.get_value() {
                counted.set_value(false);
                u.modals.update(|n| *n = n.saturating_sub(1));
                // Restore focus on the next frame: the shell is still `inert` right now.
                if let Some(el) = restore.get_value() {
                    request_animation_frame(move || {
                        let _ = el.focus();
                    });
                }
            }
        });
    }
    on_cleanup(move || {
        if counted.try_get_value().unwrap_or(false) {
            u.modals.try_update(|n| *n = n.saturating_sub(1));
        }
    });
    let class = if wide { "modal modal-wide" } else { "modal" };
    move || {
        let dialog_id = dialog_id.clone();
        let title_id = title_id.clone();
        open.get().then(move || {
            view! {
                <Portal>
                    <div
                        class="modal-backdrop"
                        on:mousedown=move |ev| {
                            if ev.target() == ev.current_target() {
                                on_close.run(());
                            }
                        }
                    >
                        <div
                            class=class
                            role="dialog"
                            aria-modal="true"
                            aria-labelledby=title_id.clone()
                            id=dialog_id.clone()
                            tabindex="-1"
                            data-testid=testid
                            on:keydown=move |ev| {
                                if ev.key() == "Escape" {
                                    ev.prevent_default();
                                    ev.stop_propagation();
                                    on_close.run(());
                                }
                            }
                        >
                            <div class="modal-header">
                                <h2 id=title_id.clone()>{move || title.get()}</h2>
                                <button
                                    type="button"
                                    class="icon-btn"
                                    data-close=""
                                    aria-label="Close dialog"
                                    on:click=move |_| on_close.run(())
                                >
                                    {icon(Icon::Close)}
                                </button>
                            </div>
                            <div class="modal-body">{children.with_value(|c| c())}</div>
                        </div>
                    </div>
                </Portal>
            }
        })
    }
}

/// Renders the global confirmation dialog (`Ui::ask`).
#[component]
pub(crate) fn ConfirmHost() -> impl IntoView {
    let u = ui();
    let open = Signal::derive(move || u.confirm.with(Option::is_some));
    let close = Callback::new(move |()| u.confirm.set(None));
    let title = Signal::derive(move || {
        u.confirm
            .with(|c| c.as_ref().map(|c| c.title.clone()).unwrap_or_default())
    });
    view! {
        <Modal open=open on_close=close title=title testid="confirm-dialog">
            {move || {
                u.confirm
                    .get()
                    .map(|req| {
                        let on_confirm = req.on_confirm;
                        let btn = if req.danger { "btn btn-danger" } else { "btn btn-primary" };
                        view! {
                            <p class="confirm-body">{req.body.clone()}</p>
                            <div class="modal-actions">
                                <button
                                    type="button"
                                    class="btn btn-secondary"
                                    data-autofocus=req.danger.then_some("")
                                    on:click=move |_| close.run(())
                                >
                                    "Cancel"
                                </button>
                                <button
                                    type="button"
                                    class=btn
                                    data-autofocus=(!req.danger).then_some("")
                                    data-testid="confirm-accept"
                                    on:click=move |_| {
                                        close.run(());
                                        on_confirm.run(());
                                    }
                                >
                                    {req.confirm_label.clone()}
                                </button>
                            </div>
                        }
                    })
            }}
        </Modal>
    }
}
