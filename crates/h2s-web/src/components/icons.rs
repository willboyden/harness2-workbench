//! A small set of line icons drawn from basic SVG shapes (decorative: `aria-hidden`).

use leptos::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Icon {
    Dashboard,
    Flask,
    Jobs,
    Plus,
    Compare,
    Box,
    Gear,
    Help,
    Menu,
    Sun,
    Moon,
    Monitor,
    Search,
    Close,
    Logout,
    Play,
    Download,
    Check,
    Alert,
    Sparkle,
    File,
    Folder,
    Refresh,
}

/// Render an icon at the current font size.
pub(crate) fn icon(i: Icon) -> impl IntoView {
    let shapes = match i {
        Icon::Dashboard => view! {
            <rect x="3" y="3" width="7" height="9" rx="1.5"></rect>
            <rect x="14" y="3" width="7" height="5" rx="1.5"></rect>
            <rect x="14" y="12" width="7" height="9" rx="1.5"></rect>
            <rect x="3" y="16" width="7" height="5" rx="1.5"></rect>
        }
        .into_any(),
        Icon::Flask => view! {
            <path d="M9 3h6"></path>
            <path d="M10 3v6l-5 9a2 2 0 0 0 1.7 3h10.6a2 2 0 0 0 1.7-3l-5-9V3"></path>
            <path d="M7.5 15h9"></path>
        }
        .into_any(),
        Icon::Jobs => view! {
            <path d="M9 6h11"></path>
            <path d="M9 12h11"></path>
            <path d="M9 18h11"></path>
            <circle cx="4.5" cy="6" r="1.2"></circle>
            <circle cx="4.5" cy="12" r="1.2"></circle>
            <circle cx="4.5" cy="18" r="1.2"></circle>
        }
        .into_any(),
        Icon::Plus => view! {
            <path d="M12 5v14"></path>
            <path d="M5 12h14"></path>
        }
        .into_any(),
        Icon::Compare => view! {
            <path d="M12 3v18"></path>
            <path d="M5 7h14"></path>
            <path d="M5 7l-3 7a3 3 0 0 0 6 0z"></path>
            <path d="M19 7l-3 7a3 3 0 0 0 6 0z"></path>
            <path d="M8 21h8"></path>
        }
        .into_any(),
        Icon::Box => view! {
            <path d="M3 7.5 12 3l9 4.5v9L12 21l-9-4.5z"></path>
            <path d="M3 7.5 12 12l9-4.5"></path>
            <path d="M12 12v9"></path>
        }
        .into_any(),
        Icon::Gear => view! {
            <circle cx="12" cy="12" r="3"></circle>
            <path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M4.9 19.1 7 17M17 7l2.1-2.1"></path>
        }
        .into_any(),
        Icon::Help => view! {
            <circle cx="12" cy="12" r="9"></circle>
            <path d="M9.5 9.5a2.5 2.5 0 1 1 3.5 2.3c-.7.3-1 1-1 1.7v.5"></path>
            <circle cx="12" cy="17" r="0.6"></circle>
        }
        .into_any(),
        Icon::Menu => view! {
            <path d="M4 6h16"></path>
            <path d="M4 12h16"></path>
            <path d="M4 18h16"></path>
        }
        .into_any(),
        Icon::Sun => view! {
            <circle cx="12" cy="12" r="4"></circle>
            <path d="M12 2v2M12 20v2M2 12h2M20 12h2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"></path>
        }
        .into_any(),
        Icon::Moon => view! {
            <path d="M20 14.5A8 8 0 1 1 9.5 4a6.5 6.5 0 0 0 10.5 10.5z"></path>
        }
        .into_any(),
        Icon::Monitor => view! {
            <rect x="3" y="4" width="18" height="12" rx="1.5"></rect>
            <path d="M8 20h8"></path>
            <path d="M12 16v4"></path>
        }
        .into_any(),
        Icon::Search => view! {
            <circle cx="11" cy="11" r="6.5"></circle>
            <path d="m16 16 4.5 4.5"></path>
        }
        .into_any(),
        Icon::Close => view! {
            <path d="M6 6l12 12"></path>
            <path d="M18 6 6 18"></path>
        }
        .into_any(),
        Icon::Logout => view! {
            <path d="M15 4h3a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2h-3"></path>
            <path d="M10 17l-5-5 5-5"></path>
            <path d="M5 12h11"></path>
        }
        .into_any(),
        Icon::Play => view! { <path d="M7 4.5v15l12-7.5z"></path> }.into_any(),
        Icon::Download => view! {
            <path d="M12 4v11"></path>
            <path d="m7 10 5 5 5-5"></path>
            <path d="M5 20h14"></path>
        }
        .into_any(),
        Icon::Check => view! { <path d="m5 12.5 4.5 4.5L19 7.5"></path> }.into_any(),
        Icon::Alert => view! {
            <path d="M12 3 2.5 20h19z"></path>
            <path d="M12 10v4.5"></path>
            <circle cx="12" cy="17.2" r="0.6"></circle>
        }
        .into_any(),
        Icon::Sparkle => view! {
            <path d="M12 3v4M12 17v4M3 12h4M17 12h4"></path>
            <path d="M12 8.5 13.2 10.8 15.5 12 13.2 13.2 12 15.5 10.8 13.2 8.5 12 10.8 10.8z"></path>
        }
        .into_any(),
        Icon::File => view! {
            <path d="M6 3h8l4 4v14H6z"></path>
            <path d="M14 3v4h4"></path>
        }
        .into_any(),
        Icon::Folder => view! {
            <path d="M3 6.5A1.5 1.5 0 0 1 4.5 5H9l2 2.5h8.5A1.5 1.5 0 0 1 21 9v9.5a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 18.5z"></path>
        }
        .into_any(),
        Icon::Refresh => view! {
            <path d="M20 11a8 8 0 0 0-14.5-4.5L4 8"></path>
            <path d="M4 4v4h4"></path>
            <path d="M4 13a8 8 0 0 0 14.5 4.5L20 16"></path>
            <path d="M20 20v-4h-4"></path>
        }
        .into_any(),
    };
    view! {
        <svg
            class="icon"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="1.8"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
            focusable="false"
        >
            {shapes}
        </svg>
    }
}
