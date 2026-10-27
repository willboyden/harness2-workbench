//! Pure logic behind the pages: formatting, validation mirrors, URL building and view-model
//! derivations. Nothing here touches the browser, so all of it is unit-tested natively with
//! `cargo test -p h2w-web`; the components stay thin.

pub(crate) mod benchmarks;
pub(crate) mod diff;
pub(crate) mod format;
pub(crate) mod glossary;
pub(crate) mod harness;
pub(crate) mod jobs;
pub(crate) mod judge;
pub(crate) mod logbuf;
pub(crate) mod names;
pub(crate) mod palette;
pub(crate) mod profile;
pub(crate) mod report;
pub(crate) mod tasks;
pub(crate) mod theme;
pub(crate) mod trajectory;
pub(crate) mod urls;
pub(crate) mod wizard;
