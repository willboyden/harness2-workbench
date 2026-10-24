//! Labelled form controls with help text and error messages wired for screen readers.

use leptos::prelude::*;

fn described_by(id: &str, has_help: bool, has_error: bool) -> Option<String> {
    let mut parts = Vec::new();
    if has_help {
        parts.push(format!("{id}-help"));
    }
    if has_error {
        parts.push(format!("{id}-error"));
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// Help line under a control.
#[component]
fn Help(id: String, #[prop(into)] text: MaybeProp<String>) -> impl IntoView {
    move || {
        text.get()
            .map(|t| view! { <p class="field-help" id=format!("{id}-help")>{t}</p> })
    }
}

/// Error line under a control (announced when it appears).
#[component]
fn FieldError(id: String, #[prop(into)] error: Signal<Option<String>>) -> impl IntoView {
    move || {
        error.get().map(|e| {
            view! {
                <p class="field-error" id=format!("{id}-error") role="alert">
                    {e}
                </p>
            }
        })
    }
}

#[component]
pub(crate) fn TextField(
    #[prop(into)] id: String,
    #[prop(into)] label: String,
    value: RwSignal<String>,
    #[prop(optional, into)] help: MaybeProp<String>,
    #[prop(optional, into)] error: Option<Signal<Option<String>>>,
    #[prop(optional, into)] placeholder: Option<String>,
    #[prop(default = "text")] kind: &'static str,
    #[prop(optional)] required: bool,
    #[prop(optional, into)] autocomplete: Option<String>,
    #[prop(optional)] testid: Option<&'static str>,
    #[prop(optional, into)] disabled: Option<Signal<bool>>,
    #[prop(optional, into)] inputmode: Option<String>,
) -> impl IntoView {
    let error = error.unwrap_or_else(|| Signal::stored(None));
    let has_help = help.get_untracked().is_some();
    let id2 = id.clone();
    let id3 = id.clone();
    let id4 = id.clone();
    view! {
        <div class="field" class:has-error=move || error.get().is_some()>
            <label for=id.clone()>
                {label}
                {required.then(|| view! { <span class="required" aria-hidden="true">" *"</span> })}
            </label>
            <input
                id=id
                type=kind
                class="input"
                bind:value=value
                placeholder=placeholder
                required=required
                autocomplete=autocomplete
                inputmode=inputmode
                spellcheck="false"
                data-testid=testid
                disabled=move || disabled.is_some_and(|d| d.get())
                aria-invalid=move || if error.get().is_some() { "true" } else { "false" }
                aria-describedby=move || described_by(&id2, has_help, error.get().is_some())
            />
            <Help id=id3 text=help />
            <FieldError id=id4 error=error />
        </div>
    }
}

#[component]
pub(crate) fn TextArea(
    #[prop(into)] id: String,
    #[prop(into)] label: String,
    value: RwSignal<String>,
    #[prop(optional, into)] help: MaybeProp<String>,
    #[prop(optional, into)] error: Option<Signal<Option<String>>>,
    #[prop(default = 4)] rows: u32,
    #[prop(optional, into)] placeholder: Option<String>,
    #[prop(optional)] monospace: bool,
) -> impl IntoView {
    let error = error.unwrap_or_else(|| Signal::stored(None));
    let has_help = help.get_untracked().is_some();
    let id2 = id.clone();
    let id3 = id.clone();
    let id4 = id.clone();
    view! {
        <div class="field" class:has-error=move || error.get().is_some()>
            <label for=id.clone()>{label}</label>
            <textarea
                id=id
                class=if monospace { "input mono" } else { "input" }
                rows=rows
                placeholder=placeholder
                spellcheck="false"
                bind:value=value
                aria-invalid=move || if error.get().is_some() { "true" } else { "false" }
                aria-describedby=move || described_by(&id2, has_help, error.get().is_some())
            ></textarea>
            <Help id=id3 text=help />
            <FieldError id=id4 error=error />
        </div>
    }
}

/// A `<select>` bound to a string signal. `options` are `(value, label)` pairs.
#[component]
pub(crate) fn SelectField(
    #[prop(into)] id: String,
    #[prop(into)] label: String,
    value: RwSignal<String>,
    #[prop(into)] options: Signal<Vec<(String, String)>>,
    #[prop(optional, into)] help: MaybeProp<String>,
    #[prop(optional, into)] error: Option<Signal<Option<String>>>,
    #[prop(optional)] testid: Option<&'static str>,
    #[prop(optional, into)] disabled: Option<Signal<bool>>,
) -> impl IntoView {
    let error = error.unwrap_or_else(|| Signal::stored(None));
    let has_help = help.get_untracked().is_some();
    let id2 = id.clone();
    let id3 = id.clone();
    let id4 = id.clone();
    view! {
        <div class="field" class:has-error=move || error.get().is_some()>
            <label for=id.clone()>{label}</label>
            <select
                id=id
                class="input"
                data-testid=testid
                disabled=move || disabled.is_some_and(|d| d.get())
                aria-describedby=move || described_by(&id2, has_help, error.get().is_some())
                on:change=move |ev| value.set(event_target_value(&ev))
                prop:value=move || value.get()
            >
                {move || {
                    let current = value.get();
                    options
                        .get()
                        .into_iter()
                        .map(|(v, l)| {
                            let selected = v == current;
                            view! {
                                <option value=v selected=selected>
                                    {l}
                                </option>
                            }
                        })
                        .collect_view()
                }}
            </select>
            <Help id=id3 text=help />
            <FieldError id=id4 error=error />
        </div>
    }
}

/// A checkbox with its label to the right and optional help below.
#[component]
pub(crate) fn Checkbox(
    #[prop(into)] id: String,
    #[prop(into)] label: String,
    checked: RwSignal<bool>,
    #[prop(optional, into)] help: MaybeProp<String>,
    #[prop(optional, into)] disabled: Option<Signal<bool>>,
    #[prop(optional)] testid: Option<&'static str>,
) -> impl IntoView {
    let has_help = help.get_untracked().is_some();
    let id2 = id.clone();
    let id3 = id.clone();
    view! {
        <div class="field field-check">
            <input
                id=id.clone()
                type="checkbox"
                bind:checked=checked
                data-testid=testid
                disabled=move || disabled.is_some_and(|d| d.get())
                aria-describedby=described_by(&id2, has_help, false)
            />
            <label for=id>{label}</label>
            <Help id=id3 text=help />
        </div>
    }
}

/// Radio cards: one choice among a few, each with a title and an explanation.
#[component]
pub(crate) fn RadioCards(
    #[prop(into)] name: String,
    #[prop(into)] legend: String,
    value: RwSignal<String>,
    /// `(value, title, description)`.
    options: Vec<(String, String, String)>,
    #[prop(optional)] compact: bool,
) -> impl IntoView {
    view! {
        <fieldset class=if compact { "radio-cards compact" } else { "radio-cards" }>
            <legend>{legend}</legend>
            {options
                .into_iter()
                .map(|(v, title, desc)| {
                    let id = format!("{name}-{v}");
                    let label_for = id.clone();
                    let v2 = v.clone();
                    let v3 = v.clone();
                    view! {
                        <label class="radio-card" class:selected=move || value.get() == v3 for=label_for>
                            <input
                                type="radio"
                                id=id
                                name=name.clone()
                                value=v.clone()
                                prop:checked=move || value.get() == v
                                on:change=move |_| value.set(v2.clone())
                            />
                            <span class="radio-card-title">{title}</span>
                            <span class="radio-card-desc">{desc}</span>
                        </label>
                    }
                })
                .collect_view()}
        </fieldset>
    }
}
