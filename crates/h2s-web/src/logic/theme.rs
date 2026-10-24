//! The colour theme preference: light, dark, or follow the system.

/// `localStorage` key holding the preference.
pub(crate) const STORAGE_KEY: &str = "h2s-theme";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Theme {
    Light,
    Dark,
    #[default]
    System,
}

impl Theme {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Theme::Light => "light",
            Theme::Dark => "dark",
            Theme::System => "system",
        }
    }

    /// Unknown or missing values mean "follow the system".
    pub(crate) fn parse(s: Option<&str>) -> Theme {
        match s {
            Some("light") => Theme::Light,
            Some("dark") => Theme::Dark,
            _ => Theme::System,
        }
    }

    /// The toggle cycles light -> dark -> system -> light.
    pub(crate) fn next(self) -> Theme {
        match self {
            Theme::Light => Theme::Dark,
            Theme::Dark => Theme::System,
            Theme::System => Theme::Light,
        }
    }

    /// Value for the `data-theme` attribute on `<html>`; `None` lets CSS follow the system.
    pub(crate) fn attribute(self) -> Option<&'static str> {
        match self {
            Theme::Light => Some("light"),
            Theme::Dark => Some("dark"),
            Theme::System => None,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Theme::Light => "Light theme",
            Theme::Dark => "Dark theme",
            Theme::System => "System theme",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_cycle() {
        assert_eq!(Theme::parse(Some("dark")), Theme::Dark);
        assert_eq!(Theme::parse(Some("garbage")), Theme::System);
        assert_eq!(Theme::parse(None), Theme::System);
        let mut t = Theme::Light;
        for _ in 0..3 {
            t = t.next();
        }
        assert_eq!(t, Theme::Light);
        for t in [Theme::Light, Theme::Dark, Theme::System] {
            assert_eq!(Theme::parse(Some(t.as_str())), t);
        }
        assert_eq!(Theme::System.attribute(), None);
    }
}
