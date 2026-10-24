//! App-wide reactive state, provided as contexts at the root: the session, toasts, global
//! dialogs and the theme.

use std::time::Duration;

use h2w_api::{Principal, Role, SystemInfo};
use leptos::prelude::*;

use crate::api::ApiError;
use crate::browser;
use crate::logic::jobs::Tone;
use crate::logic::theme::{self, Theme};
use crate::logic::wizard::HostRisk;

/// Who is signed in, and what the server is.
#[derive(Clone, Copy)]
pub(crate) struct Session {
    pub(crate) principal: RwSignal<Option<Principal>>,
    /// `GET /auth/me` has answered (either way).
    pub(crate) checked: RwSignal<bool>,
    /// `GET /system/info`, loaded after sign-in.
    pub(crate) system: RwSignal<Option<SystemInfo>>,
}

impl Session {
    /// Reactive: true when the signed-in role allows `required`.
    pub(crate) fn can(&self, required: Role) -> bool {
        self.principal
            .with(|p| p.as_ref().is_some_and(|p| p.role.allows(required)))
    }

    pub(crate) fn role(&self) -> Option<Role> {
        self.principal.with(|p| p.as_ref().map(|p| p.role))
    }

    /// Reactive: what a run asks about host isolation (an unknown server asks).
    pub(crate) fn host_risk(&self) -> HostRisk {
        self.system.with(|s| match s {
            None => HostRisk::AskUser,
            Some(s) => HostRisk::from_system(
                s.isolation == h2w_api::Isolation::Host,
                s.host_risks_preacknowledged,
            ),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Toast {
    pub(crate) id: u64,
    pub(crate) tone: Tone,
    pub(crate) title: String,
    pub(crate) body: String,
    /// Optional `(label, href)` link.
    pub(crate) link: Option<(String, String)>,
}

#[derive(Clone, Copy)]
pub(crate) struct Toasts {
    pub(crate) list: RwSignal<Vec<Toast>>,
    next: RwSignal<u64>,
}

impl Toasts {
    fn new() -> Toasts {
        Toasts {
            list: RwSignal::new(Vec::new()),
            next: RwSignal::new(1),
        }
    }

    pub(crate) fn push(&self, tone: Tone, title: &str, body: &str, link: Option<(String, String)>) {
        let id = self.next.get_untracked();
        self.next.set(id + 1);
        self.list.update(|l| {
            l.push(Toast {
                id,
                tone,
                title: title.to_owned(),
                body: body.to_owned(),
                link,
            });
            if l.len() > 4 {
                l.remove(0);
            }
        });
        let list = self.list;
        let ms = if tone == Tone::Danger { 12_000 } else { 6_000 };
        set_timeout(
            move || list.update(|l| l.retain(|t| t.id != id)),
            Duration::from_millis(ms),
        );
    }

    pub(crate) fn success(&self, title: &str) {
        self.push(Tone::Success, title, "", None);
    }

    pub(crate) fn info(&self, title: &str) {
        self.push(Tone::Info, title, "", None);
    }

    pub(crate) fn error(&self, title: &str, err: &ApiError) {
        // A 401 already sends the user to sign in; no need for a second message.
        if !err.is_unauthorized() {
            let body = match &err.request_id {
                Some(id) => format!("{err} (request id {id})"),
                None => err.to_string(),
            };
            self.push(Tone::Danger, title, &body, None);
        }
    }

    pub(crate) fn warn(&self, title: &str, body: &str) {
        self.push(Tone::Warning, title, body, None);
    }

    pub(crate) fn dismiss(&self, id: u64) {
        self.list.update(|l| l.retain(|t| t.id != id));
    }
}

/// A confirmation the user must accept before something irreversible or costly happens.
#[derive(Clone)]
pub(crate) struct ConfirmRequest {
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) confirm_label: String,
    pub(crate) danger: bool,
    pub(crate) on_confirm: Callback<()>,
}

/// Global UI state: dialogs, palette, navigation drawer, theme and a coarse clock.
#[derive(Clone, Copy)]
pub(crate) struct Ui {
    pub(crate) confirm: RwSignal<Option<ConfirmRequest>>,
    pub(crate) palette: RwSignal<bool>,
    pub(crate) shortcuts: RwSignal<bool>,
    /// Page-level dialogs currently open (each `Modal` counts itself).
    pub(crate) modals: RwSignal<u32>,
    /// The sidebar drawer on narrow screens.
    pub(crate) nav_open: RwSignal<bool>,
    pub(crate) theme: RwSignal<Theme>,
    /// Unix seconds, refreshed every 30 s so relative times stay current.
    pub(crate) now: RwSignal<i64>,
}

impl Ui {
    fn new() -> Ui {
        let theme = Theme::parse(browser::storage_get(theme::STORAGE_KEY).as_deref());
        browser::apply_theme(theme.attribute());
        let now = RwSignal::new(browser::now_secs());
        set_interval(
            move || now.set(browser::now_secs()),
            Duration::from_secs(30),
        );
        Ui {
            confirm: RwSignal::new(None),
            palette: RwSignal::new(false),
            shortcuts: RwSignal::new(false),
            modals: RwSignal::new(0),
            nav_open: RwSignal::new(false),
            theme: RwSignal::new(theme),
            now,
        }
    }

    pub(crate) fn set_theme(&self, t: Theme) {
        self.theme.set(t);
        browser::apply_theme(t.attribute());
        browser::storage_set(theme::STORAGE_KEY, t.as_str());
    }

    pub(crate) fn ask(&self, req: ConfirmRequest) {
        self.confirm.set(Some(req));
    }

    /// True while any modal surface is open (the page behind it is made inert).
    pub(crate) fn modal_open(&self) -> bool {
        self.modals.get() > 0 || self.palette.get() || self.shortcuts.get()
    }
}

/// Create and provide every root context. Called once by the app root.
pub(crate) fn provide_all() {
    provide_context(Session {
        principal: RwSignal::new(None),
        checked: RwSignal::new(false),
        system: RwSignal::new(None),
    });
    provide_context(Toasts::new());
    provide_context(Ui::new());
}

pub(crate) fn session() -> Session {
    expect_context::<Session>()
}

pub(crate) fn toasts() -> Toasts {
    expect_context::<Toasts>()
}

pub(crate) fn ui() -> Ui {
    expect_context::<Ui>()
}
