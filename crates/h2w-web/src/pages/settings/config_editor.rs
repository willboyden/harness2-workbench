//! The server configuration editor (administrators): draft, validate and export an
//! `h2workbench.toml`.
//!
//! It never changes the server. Loading reads the server's file read-only (the server refuses
//! one that looks like it holds a credential), the shipped example, or a local file read in
//! the browser. The Form and Text views edit one draft; Validate sends the text to the
//! server's own loader, which writes nothing; Export gives a diff, Copy, Download and the
//! commands an administrator runs to install the file and restart.

use h2w_api::server_config::{
    CONFIG_KEYS, CONFIG_SECTIONS, ConfigKeyHelp, ConfigSectionHelp, ConfigValueKind,
};
use h2w_api::{ConfigValidation, ServerConfigFile, ValidateConfigRequest};
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::api::{self, ApiError};
use crate::browser;
use crate::components::icons::{Icon, icon};
use crate::components::info::{Info, InfoButton, InfoPop};
use crate::components::page_help::rich_text;
use crate::components::ui::{Badge, Card, CodeBlock, CopyButton, ErrorPanel, Notice};
use crate::logic::cfgtext::{self, Scan};
use crate::logic::diff::{LineKind, line_diff};
use crate::logic::jobs::Tone;
use crate::logic::urls::api as paths;
use crate::state::{ConfirmRequest, toasts, ui};

/// The annotated example that ships with the Workbench.
const EXAMPLE: &str = include_str!("../../../../../deploy/examples/h2workbench.toml");

/// Largest local file opened into the editor (the server validates at most this much).
const OPEN_MAX_BYTES: f64 = 256.0 * 1024.0;

/// What the draft was loaded from: the diff's left side.
#[derive(Clone, Debug, PartialEq)]
struct Base {
    label: String,
    text: String,
}

/// The editor's shared state.
#[derive(Clone, Copy)]
struct Draft {
    text: RwSignal<String>,
    base: RwSignal<Option<Base>>,
    scan: Memo<Scan>,
}

impl Draft {
    fn set(self, path: &str, literal: &str) {
        self.text.update(|t| *t = cfgtext::set(t, path, literal));
    }

    fn reset(self, path: &str) {
        self.text.update(|t| *t = cfgtext::remove(t, path));
    }

    fn dirty(self) -> bool {
        let text = self.text.get_untracked();
        self.base
            .with_untracked(|b| b.as_ref().map_or(!text.is_empty(), |b| b.text != text))
    }

    /// Replace the draft, asking first when it has unexported edits.
    fn load(self, label: String, text: String) {
        let apply = move || {
            self.text.set(text.clone());
            self.base.set(Some(Base {
                label: label.clone(),
                text: text.clone(),
            }));
        };
        if self.dirty() {
            ui().ask(ConfirmRequest {
                title: "Replace the draft?".to_owned(),
                body: "The draft has changes that are not exported. Loading replaces them."
                    .to_owned(),
                confirm_label: "Replace".to_owned(),
                danger: true,
                on_confirm: Callback::new(move |()| apply()),
            });
        } else {
            apply();
        }
    }
}

fn row_id(path: &str) -> String {
    format!("cfg-{}", path.replace('.', "-"))
}

fn key_name(path: &str) -> &str {
    path.rsplit('.').next().unwrap_or(path)
}

/// The (i) pop-up text of a key: what it is, what changing it does, values, when it applies.
fn key_help(k: &'static ConfigKeyHelp) -> impl IntoView {
    view! {
        <div class="cfg-help">
            {k.security
                .then(|| {
                    view! {
                        <p class="cfg-help-sec">
                            <Badge tone=Tone::Warning text="Security" />
                            " This key affects who can use the server or what jobs can do and reach. Read “Changing it” first."
                        </p>
                    }
                })}
            <h4>"What it is"</h4>
            {rich_text(k.what, "")}
            <h4>"Changing it"</h4>
            {rich_text(k.effect, "")}
            <h4>"Values and default"</h4>
            {rich_text(k.values, "")}
            <h4>"When it takes effect"</h4>
            {rich_text(k.applies, "")}
        </div>
    }
}

/// The value a control shows: the text's value when the form can read it, else the default.
#[derive(Clone, Debug, PartialEq)]
enum Shown {
    /// Set in the draft, readable by the form.
    Set(String),
    /// Not in the draft: the default applies.
    Default(String),
    /// Set in a form the form cannot show (edit it in the Text view).
    Unreadable,
}

fn readable(kind: ConfigValueKind, raw: &str) -> Option<String> {
    match kind {
        ConfigValueKind::Bool => cfgtext::parse_bool(raw).map(|b| b.to_string()),
        ConfigValueKind::Integer => cfgtext::parse_int(raw).map(|i| i.to_string()),
        ConfigValueKind::Float => cfgtext::parse_float(raw).map(|_| raw.trim().replace('_', "")),
        ConfigValueKind::String | ConfigValueKind::Enum(_) => cfgtext::unquote(raw),
        ConfigValueKind::StringList => cfgtext::string_list(raw).map(|l| l.join("\n")),
        ConfigValueKind::Complex => Some(raw.to_owned()),
    }
}

fn shown(scan: &Scan, k: &ConfigKeyHelp) -> Shown {
    match scan.get(k.path) {
        Some(e) => readable(k.kind, &e.raw).map_or(Shown::Unreadable, Shown::Set),
        None => Shown::Default(readable(k.kind, k.default).unwrap_or_default()),
    }
}

/// The TOML literal for what the user typed, or why it is not a valid value of this kind.
fn literal(kind: ConfigValueKind, input: &str) -> Result<String, String> {
    let v = input.trim();
    match kind {
        ConfigValueKind::Integer => cfgtext::parse_int(v)
            .filter(|i| *i >= 0)
            .map(|i| i.to_string())
            .ok_or_else(|| "Enter a whole number, 0 or more.".to_owned()),
        ConfigValueKind::Float => cfgtext::parse_float(v)
            .map(|f| {
                let s = f.to_string();
                if s.contains(['.', 'e', 'E']) {
                    s
                } else {
                    format!("{s}.0")
                }
            })
            .ok_or_else(|| "Enter a number, such as 4 or 0.5.".to_owned()),
        ConfigValueKind::String | ConfigValueKind::Enum(_) => Ok(cfgtext::quote(input)),
        ConfigValueKind::Bool => cfgtext::parse_bool(v)
            .map(|b| b.to_string())
            .ok_or_else(|| "true or false".to_owned()),
        ConfigValueKind::StringList => Ok(cfgtext::render_string_list(
            &input
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>(),
        )),
        ConfigValueKind::Complex => {
            if v.is_empty() {
                Err("Empty: use “Use the default” to remove the key.".to_owned())
            } else {
                Ok(v.to_owned())
            }
        }
    }
}

fn event_value(ev: &leptos::ev::Event) -> String {
    event_target_value(ev)
}

/// One key: label, (i), control, default and reset.
#[component]
fn KeyRow(k: &'static ConfigKeyHelp, draft: Draft, filter: RwSignal<String>) -> impl IntoView {
    let id = row_id(k.path);
    let info = Info::new(&id);
    let error = RwSignal::new(None::<String>);
    let state = Memo::new(move |_| draft.scan.with(|s| shown(s, k)));
    let is_set = Signal::derive(move || !matches!(state.get(), Shown::Default(_)));
    let value = Signal::derive(move || match state.get() {
        Shown::Set(v) | Shown::Default(v) => v,
        Shown::Unreadable => String::new(),
    });
    let unreadable = Signal::derive(move || state.get() == Shown::Unreadable);
    let hidden = Signal::derive(move || {
        let q = filter.get().trim().to_lowercase();
        !q.is_empty()
            && !q
                .split_whitespace()
                .all(|w| k.path.contains(w) || k.what.to_lowercase().contains(w))
    });
    let commit = move |input: String| match literal(k.kind, &input) {
        Ok(lit) => {
            error.set(None);
            draft.set(k.path, &lit);
        }
        Err(e) => error.set(Some(e)),
    };
    let described = format!("{} {id}-meta", info.pop_id());
    let control = match k.kind {
        ConfigValueKind::Bool => view! {
            <label class="cfg-check">
                <input
                    type="checkbox"
                    id=id.clone()
                    prop:checked=move || value.get() == "true"
                    disabled=unreadable
                    aria-describedby=described
                    on:change=move |ev| {
                        let on = event_target_checked(&ev);
                        draft.set(k.path, if on { "true" } else { "false" });
                    }
                />
                <span>{move || if value.get() == "true" { "true" } else { "false" }}</span>
            </label>
        }
        .into_any(),
        ConfigValueKind::Enum(opts) => view! {
            <select
                class="input"
                id=id.clone()
                disabled=unreadable
                aria-describedby=described
                prop:value=move || value.get()
                on:change=move |ev| commit(event_value(&ev))
            >
                {opts
                    .iter()
                    .map(|o| view! { <option value=*o selected=move || value.get() == *o>{*o}</option> })
                    .collect_view()}
            </select>
        }
        .into_any(),
        ConfigValueKind::StringList | ConfigValueKind::Complex => {
            let mono = matches!(k.kind, ConfigValueKind::Complex);
            view! {
                <textarea
                    class=if mono { "input mono" } else { "input" }
                    id=id.clone()
                    rows=3
                    spellcheck="false"
                    disabled=unreadable
                    aria-invalid=move || if error.get().is_some() { "true" } else { "false" }
                    aria-describedby=described
                    placeholder=if mono { "TOML value" } else { "One per line" }
                    prop:value=move || value.get()
                    on:change=move |ev| commit(event_value(&ev))
                ></textarea>
            }
            .into_any()
        }
        _ => view! {
            <input
                class="input mono"
                type="text"
                id=id.clone()
                spellcheck="false"
                autocomplete="off"
                inputmode=matches!(k.kind, ConfigValueKind::Integer | ConfigValueKind::Float).then_some("decimal")
                disabled=unreadable
                aria-invalid=move || if error.get().is_some() { "true" } else { "false" }
                aria-describedby=described
                prop:value=move || value.get()
                on:change=move |ev| commit(event_value(&ev))
            />
        }
        .into_any(),
    };
    let reset_label = format!("Use the default for {}", k.path);
    view! {
        <div class="cfg-row" class:is-set=is_set hidden=hidden data-testid="cfg-row" data-key=k.path>
            <div class="label-row">
                <label for=id.clone()>
                    <code>{key_name(k.path)}</code>
                </label>
                <InfoButton info=info label=k.path.to_owned() />
                {k.security.then(|| view! { <Badge tone=Tone::Warning text="security" /> })}
                {move || if is_set.get() {
                    view! { <Badge tone=Tone::Info text="set" /> }.into_any()
                } else {
                    view! { <Badge tone=Tone::Neutral text="default" /> }.into_any()
                }}
            </div>
            <InfoPop info=info title=k.path.to_owned() wide=true>
                {key_help(k)}
            </InfoPop>
            {control}
            <p class="cfg-meta" id=format!("{id}-meta")>
                {(!k.default.is_empty()).then(|| view! { "Default " <code>{k.default}</code> })}
                {move || unreadable.get().then(|| view! { <span class="cfg-warn">" Written in a form this view cannot show: edit it in the Text view."</span> })}
                {move || {
                    is_set
                        .get()
                        .then(|| {
                            view! {
                                " "
                                <button
                                    type="button"
                                    class="link-btn"
                                    aria-label=reset_label.clone()
                                    on:click=move |_| {
                                        error.set(None);
                                        draft.reset(k.path);
                                    }
                                >
                                    "Use the default"
                                </button>
                            }
                        })
                }}
            </p>
            {move || error.get().map(|e| view! { <p class="field-error" role="alert">{e}</p> })}
        </div>
    }
}

/// One `[table]`: its keys, with the table's own explanation.
#[component]
fn Section(s: &'static ConfigSectionHelp, draft: Draft, filter: RwSignal<String>) -> impl IntoView {
    let keys: Vec<&'static ConfigKeyHelp> = CONFIG_KEYS
        .iter()
        .filter(|k| k.path.rsplit_once('.').is_some_and(|(t, _)| t == s.path))
        .collect();
    let paths: Vec<&'static str> = keys.iter().map(|k| k.path).collect();
    let all_hidden = Signal::derive(move || {
        let q = filter.get().trim().to_lowercase();
        !q.is_empty()
            && !paths.iter().any(|p| {
                q.split_whitespace().all(|w| {
                    p.contains(w)
                        || CONFIG_KEYS
                            .iter()
                            .find(|k| k.path == *p)
                            .is_some_and(|k| k.what.to_lowercase().contains(w))
                })
            })
    });
    let table_array = Signal::derive(move || {
        draft.scan.with(|sc| {
            sc.table_arrays.iter().any(|t| {
                t.starts_with(s.path) && t.rsplit_once('.').is_some_and(|(p, _)| p == s.path)
            })
        })
    });
    view! {
        <fieldset class="cfg-section" hidden=all_hidden>
            <legend>
                <code>{format!("[{}]", s.path)}</code>
                " "
                {s.title}
            </legend>
            {rich_text(s.about, "cfg-about")}
            {move || {
                table_array
                    .get()
                    .then(|| {
                        view! {
                            <p class="small muted">
                                "This draft also has "<code>"[[...]]"</code>
                                " tables in this section; edit those in the Text view."
                            </p>
                        }
                    })
            }}
            {keys.into_iter().map(|k| view! { <KeyRow k=k draft=draft filter=filter /> }).collect_view()}
        </fieldset>
    }
}

/// Read a local file into the draft, in the browser (never uploaded).
fn open_local_file(ev: &leptos::ev::Event, draft: Draft) {
    let t = toasts();
    let Some(input) = ev
        .target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
    else {
        return;
    };
    let Some(file) = input.files().and_then(|f| f.get(0)) else {
        return;
    };
    input.set_value("");
    if file.size() > OPEN_MAX_BYTES {
        t.warn(
            "File too large",
            "A configuration file larger than 256 KiB cannot be checked by the server.",
        );
        return;
    }
    let Ok(reader) = web_sys::FileReader::new() else {
        return;
    };
    let name = file.name();
    let r = reader.clone();
    let onload = Closure::once_into_js(move || match r.result().ok().and_then(|v| v.as_string()) {
        Some(text) => draft.load(format!("local file {name}"), text),
        None => t.warn(
            "Could not read the file",
            "The browser could not read it as text.",
        ),
    });
    reader.set_onload(Some(onload.unchecked_ref()));
    if reader.read_as_text(&file).is_err() {
        t.warn("Could not read the file", "The browser refused to read it.");
    }
}

/// The validation result, or nothing before the first check.
#[component]
fn ValidationResult(
    result: RwSignal<Option<(String, Result<ConfigValidation, ApiError>)>>,
    draft: Draft,
    go_to_line: Callback<u32>,
) -> impl IntoView {
    move || {
        let (checked, r) = result.get()?;
        let stale = draft.text.with(|t| *t != checked);
        Some(match r {
            Err(e) => view! { <ErrorPanel error=e /> }.into_any(),
            Ok(v) => {
                let stale_note = stale.then(|| {
                    view! { <p class="small muted">"The draft changed after this check. Validate again."</p> }
                });
                let warnings = (!v.warnings.is_empty()).then(|| {
                    let w = v.warnings.clone();
                    view! {
                        <Notice tone=Tone::Warning title="Warnings">
                            <ul>{w.into_iter().map(|w| view! { <li>{w}</li> }).collect_view()}</ul>
                        </Notice>
                    }
                });
                let body = if v.valid {
                    view! {
                        <Notice tone=Tone::Success title="The server's loader accepts this file">
                            <p>"It parses and passes every check the server runs at start. Environment overrides (H2W_*) are not part of this check."</p>
                        </Notice>
                    }
                    .into_any()
                } else if let Some(p) = v.parse_error.clone() {
                    let line = p.line;
                    let where_ = match (p.line, p.column) {
                        (Some(l), Some(c)) => format!("Line {l}, column {c}: "),
                        (Some(l), None) => format!("Line {l}: "),
                        _ => String::new(),
                    };
                    view! {
                        <Notice tone=Tone::Danger title="The server cannot read this file">
                            <p>{where_}{p.message}</p>
                            {line.map(|l| view! {
                                <button type="button" class="btn btn-secondary btn-sm" on:click=move |_| go_to_line.run(l)>
                                    {format!("Show line {l} in the Text view")}
                                </button>
                            })}
                        </Notice>
                    }
                    .into_any()
                } else {
                    let problems = v.problems.clone();
                    view! {
                        <Notice tone=Tone::Danger title=format!("{} problem(s): the server would refuse to start", problems.len())>
                            <ul>{problems.into_iter().map(|p| view! { <li><code>{p}</code></li> }).collect_view()}</ul>
                        </Notice>
                    }
                    .into_any()
                };
                view! { <div data-testid="cfg-validation">{stale_note}{body}{warnings}</div> }
                    .into_any()
            }
        })
    }
}

#[component]
pub(crate) fn ConfigEditor() -> impl IntoView {
    let t = toasts();
    let text = RwSignal::new(String::new());
    let base = RwSignal::new(None::<Base>);
    let scan = Memo::new(move |_| text.with(|t| cfgtext::scan(t)));
    let draft = Draft { text, base, scan };
    let server = RwSignal::new(None::<Result<ServerConfigFile, ApiError>>);
    let busy = RwSignal::new(false);
    let tab = RwSignal::new("form");
    let filter = RwSignal::new(String::new());
    let result = RwSignal::new(None::<(String, Result<ConfigValidation, ApiError>)>);
    let checking = RwSignal::new(false);

    let load_server = move |_| {
        busy.set(true);
        spawn_local(async move {
            let r = api::get::<ServerConfigFile>(&paths::system_config()).await;
            busy.set(false);
            if let Ok(f) = &r {
                if let (Some(text), Some(path)) = (f.text.clone(), f.path.clone()) {
                    draft.load(format!("the server's file {path}"), text);
                } else if f.path.is_none() {
                    t.info("The server runs on built-in defaults: there is no file to load.");
                }
            }
            server.set(Some(r));
        });
    };
    let load_example = move |_| draft.load("the shipped example".to_owned(), EXAMPLE.to_owned());
    let load_empty = move |_| draft.load("an empty file (all defaults)".to_owned(), String::new());
    let validate = move |_| {
        let body = ValidateConfigRequest {
            text: text.get_untracked(),
        };
        checking.set(true);
        spawn_local(async move {
            let r = api::post::<_, ConfigValidation>(&paths::system_config_validate(), &body).await;
            checking.set(false);
            result.set(Some((body.text, r)));
        });
    };
    let go_to_line = Callback::new(move |line: u32| {
        tab.set("text");
        let offset = text.with_untracked(|t| {
            t.split_inclusive('\n')
                .take(line.saturating_sub(1) as usize)
                .map(|l| l.encode_utf16().count())
                .sum::<usize>()
        });
        request_animation_frame(move || {
            if let Some(el) = document()
                .get_element_by_id("cfg-text")
                .and_then(|e| e.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
            {
                let _ = el.focus();
                let o = u32::try_from(offset).unwrap_or(0);
                let _ = el.set_selection_range(o, o);
            }
        });
    });
    let diff = Memo::new(move |_| {
        base.with(|b| {
            b.as_ref()
                .map(|b| text.with(|t| line_diff(&b.text, t, 3)))
                .unwrap_or_default()
        })
    });
    let server_path = Signal::derive(move || {
        server.with(|s| {
            s.as_ref()
                .and_then(|r| r.as_ref().ok())
                .and_then(|f| f.path.clone())
        })
    });
    let tab_keys = move |e: web_sys::KeyboardEvent| {
        let next = match e.key().as_str() {
            "ArrowRight" | "ArrowLeft" => {
                if tab.get_untracked() == "form" {
                    "text"
                } else {
                    "form"
                }
            }
            _ => return,
        };
        e.prevent_default();
        tab.set(next);
        browser::focus_id(if next == "form" {
            "cfg-tab-form"
        } else {
            "cfg-tab-text"
        });
    };

    view! {
        <Card title="Server configuration (h2workbench.toml)" testid="config-editor">
            <p>
                "Draft, check and export the server's configuration file. Nothing here changes the server: the Workbench never writes its configuration. You install the exported file by hand and restart the server."
            </p>
            <div class="cfg-sources" role="group" aria-label="Load a configuration">
                <button type="button" class="btn btn-secondary btn-sm" disabled=busy on:click=load_server data-testid="cfg-load-server">
                    {icon(Icon::File)}" Load the server's file"
                </button>
                <button type="button" class="btn btn-secondary btn-sm" on:click=load_example data-testid="cfg-load-example">
                    {icon(Icon::Sparkle)}" Load the annotated example"
                </button>
                <button type="button" class="btn btn-secondary btn-sm" on:click=load_empty>
                    "Start from defaults"
                </button>
                <label class="btn btn-secondary btn-sm file-btn">
                    {icon(Icon::Folder)}" Open a local file…"
                    <input
                        type="file"
                        class="visually-hidden"
                        accept=".toml,text/plain"
                        on:change=move |ev| open_local_file(&ev, draft)
                    />
                </label>
            </div>
            <p class="small muted" role="status">
                {move || {
                    base.with(|b| match b {
                        Some(b) => format!("Draft started from {}.", b.label),
                        None => "Nothing loaded yet: load a file above, or type in the Text view.".to_owned(),
                    })
                }}
            </p>
            {move || {
                server.get().map(|r| match r {
                    Err(e) => view! { <ErrorPanel error=e /> }.into_any(),
                    Ok(f) => view! {
                        {f.refused.clone().map(|why| view! {
                            <Notice tone=Tone::Warning title="The server did not send its file">
                                <p>{why}</p>
                                <p>"Fix the file on the server (secrets belong in Settings · Secrets, referenced by name), or start from the example."</p>
                            </Notice>
                        })}
                        {f.changed_since_start.then(|| view! {
                            <Notice tone=Tone::Info title="Changed since the server started">
                                <p>"The file on disk no longer gives the settings the server runs with: it was edited after the start (or cannot be read now). A restart applies it."</p>
                            </Notice>
                        })}
                        {(!f.env_overrides.is_empty()).then(|| {
                            let names = f.env_overrides.join(", ");
                            view! {
                                <p class="small">"Environment overrides in force (they win over the file): "<code>{names}</code></p>
                            }
                        })}
                    }
                    .into_any(),
                })
            }}

            <div class="tabs cfg-tabs" role="tablist" aria-label="Editor view">
                <button type="button" role="tab" class="tab" id="cfg-tab-form"
                    aria-selected=move || if tab.get() == "form" { "true" } else { "false" }
                    aria-controls="cfg-panel-form"
                    tabindex=move || if tab.get() == "form" { "0" } else { "-1" }
                    on:click=move |_| tab.set("form")
                    on:keydown=tab_keys
                >"Form"</button>
                <button type="button" role="tab" class="tab" id="cfg-tab-text"
                    aria-selected=move || if tab.get() == "text" { "true" } else { "false" }
                    aria-controls="cfg-panel-text"
                    tabindex=move || if tab.get() == "text" { "0" } else { "-1" }
                    on:click=move |_| tab.set("text")
                    on:keydown=tab_keys
                >"Text"</button>
            </div>
            <div role="tabpanel" id="cfg-panel-form" aria-labelledby="cfg-tab-form" hidden=move || tab.get() != "form">
                <div class="field cfg-filter">
                    <label for="cfg-filter">"Filter keys"</label>
                    <input id="cfg-filter" class="input" type="search" autocomplete="off" placeholder="e.g. proxy, egress, timeout" bind:value=filter />
                </div>
                <p class="small muted">
                    "Each key shows the draft's value, or its default when the draft does not set it. Changes are written into the draft when you leave a field; comments and layout are kept. The (i) button explains a key."
                </p>
                {CONFIG_SECTIONS.iter().map(|s| view! { <Section s=s draft=draft filter=filter /> }).collect_view()}
            </div>
            <div role="tabpanel" id="cfg-panel-text" aria-labelledby="cfg-tab-text" hidden=move || tab.get() != "text">
                <div class="field">
                    <label for="cfg-text">"h2workbench.toml"</label>
                    <textarea
                        id="cfg-text"
                        class="input mono cfg-text"
                        rows=24
                        spellcheck="false"
                        autocomplete="off"
                        data-testid="cfg-text"
                        prop:value=move || text.get()
                        on:input=move |ev| text.set(event_target_value(&ev))
                    ></textarea>
                </div>
            </div>

            <div class="cfg-actions">
                <button type="button" class="btn btn-primary" disabled=checking on:click=validate data-testid="cfg-validate">
                    {icon(Icon::Check)}
                    {move || if checking.get() { " Validating…" } else { " Validate with the server" }}
                </button>
                <span class="small muted">"Runs the server's own parser and checks on the text. Nothing is written."</span>
            </div>
            <ValidationResult result=result draft=draft go_to_line=go_to_line />

            <h3 class="cfg-export-title">"Export"</h3>
            {move || {
                let lines = diff.get();
                let added = lines.iter().filter(|l| l.kind == LineKind::Added).count();
                let removed = lines.iter().filter(|l| l.kind == LineKind::Removed).count();
                match base.get() {
                    None => view! { <p class="muted small">"Load a file to see what you changed."</p> }.into_any(),
                    Some(_) if lines.is_empty() => view! { <p class="muted small">"No changes to the loaded file."</p> }.into_any(),
                    Some(b) => view! {
                        <p class="small">
                            {format!("Changes against {}: ", b.label)}
                            <span class="add-count">{format!("+{added}")}</span>" "
                            <span class="del-count">{format!("\u{2212}{removed}")}</span>
                        </p>
                        <div class="diff-body cfg-diff">
                            <table class="diff-table">
                                <tbody>
                                    {lines.into_iter().map(|l| {
                                        let sign = match l.kind { LineKind::Added => "+", LineKind::Removed => "\u{2212}", _ => "" };
                                        let sr = l.kind.sr_label();
                                        view! {
                                            <tr class=l.kind.class()>
                                                <td class="ln">{l.old_no.map(|n| n.to_string()).unwrap_or_default()}</td>
                                                <td class="ln">{l.new_no.map(|n| n.to_string()).unwrap_or_default()}</td>
                                                <td class="sign"><span aria-hidden="true">{sign}</span>{(!sr.is_empty()).then(|| view! { <span class="visually-hidden">{sr}</span> })}</td>
                                                <td class="code">{l.text}</td>
                                            </tr>
                                        }
                                    }).collect_view()}
                                </tbody>
                            </table>
                        </div>
                    }.into_any(),
                }
            }}
            <div class="cfg-actions">
                <CopyButton text=Signal::derive(move || text.get()) label="Copy the draft" />
                <button
                    type="button"
                    class="btn btn-secondary btn-sm"
                    data-testid="cfg-download"
                    on:click=move |_| {
                        if !browser::download_text("h2workbench.toml", "text/plain", &text.get_untracked()) {
                            t.warn("Download failed", "The browser refused the download. Copy the text instead.");
                        }
                    }
                >
                    {icon(Icon::Download)}" Download h2workbench.toml"
                </button>
            </div>
            <p class="small">
                "To install it, run these on the server as the user the server runs as. They are shown, never run. Adapt the restart to how you run the server: with the compose deployment, restart its services with "
                <code>"docker compose restart"</code>
                " and keep the mounted file readable by the container's user, as the Administrator guide describes."
            </p>
            <CodeBlock text=Signal::derive(move || cfgtext::install_commands(server_path.get().as_deref())) copy=true label="Install commands" />
        </Card>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalog_default_is_readable_by_its_control() {
        for k in CONFIG_KEYS {
            if !k.default.is_empty() {
                assert!(readable(k.kind, k.default).is_some(), "{}", k.path);
            }
        }
    }

    #[test]
    fn every_example_value_is_readable() {
        let s = cfgtext::scan(EXAMPLE);
        for k in CONFIG_KEYS {
            assert!(!matches!(shown(&s, k), Shown::Unreadable), "{}", k.path);
        }
    }

    #[test]
    fn literals() {
        assert_eq!(
            literal(ConfigValueKind::Integer, " 12 "),
            Ok("12".to_owned())
        );
        assert!(literal(ConfigValueKind::Integer, "-1").is_err());
        assert!(literal(ConfigValueKind::Integer, "1.5").is_err());
        assert_eq!(literal(ConfigValueKind::Float, "4"), Ok("4.0".to_owned()));
        assert_eq!(literal(ConfigValueKind::Float, "0.5"), Ok("0.5".to_owned()));
        assert_eq!(
            literal(ConfigValueKind::String, "a\"b"),
            Ok("\"a\\\"b\"".to_owned())
        );
        assert_eq!(
            literal(
                ConfigValueKind::StringList,
                "a.example.com\n\n b.example.com "
            ),
            Ok("[\"a.example.com\", \"b.example.com\"]".to_owned())
        );
        assert!(literal(ConfigValueKind::Complex, "  ").is_err());
    }

    #[test]
    fn install_commands_name_the_file() {
        let c = cfgtext::install_commands(Some("/etc/h2w/h2workbench.toml"));
        assert!(c.contains("install -m 600 ./h2workbench.toml '/etc/h2w/h2workbench.toml'"));
        assert!(c.contains("config check"));
        assert!(cfgtext::install_commands(None).contains("'/path/to/the/server/h2workbench.toml'"));
    }
}
