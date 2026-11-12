//! Bridge from the existing per-platform selector strings to the shared
//! locator model.
//!
//! This does not change the selector grammar in `impresario-core`: it is a
//! read-only mapping used when a runner wants to evaluate a legacy scene cell
//! through the shared snapshot matcher. Unsupported forms (predicates and
//! arbitrary native classes) return `None` rather than guessing.

use crate::ast::{Kind, Locator};
use crate::role::{role_from_android_class, role_from_ios_type, Role};
use crate::text::NameFilter;

/// The platform family a legacy selector was written for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyPlatform {
    Web,
    Android,
    Apple,
}

/// Map one legacy selector value to a locator.
///
/// Existing locator expressions are parsed directly. Empty values and the
/// absent markers `-`, `~`, `none` and `null` return `None`.
#[must_use]
pub fn to_locator(value: &str, platform: LegacyPlatform) -> Option<Locator> {
    let value = value.trim();
    if value.is_empty() || matches!(value, "-" | "~" | "none" | "null") {
        return None;
    }
    if crate::is_locator_expression(value) {
        return crate::parse(value).ok();
    }
    if value.ends_with(':') && is_known_prefix(value.trim_end_matches(':')) {
        return None;
    }

    if let Some(rest) = strip(value, "textContains") {
        return Some(Locator::simple(Kind::Text(NameFilter::contains(
            rest, false,
        ))));
    }
    if let Some(rest) = strip(value, "text") {
        return Some(Locator::simple(Kind::Text(NameFilter::exact(rest, false))));
    }
    if let Some(rest) = strip(value, "labelContains") {
        return Some(Locator::simple(Kind::Label(NameFilter::contains(
            rest, false,
        ))));
    }
    if let Some(rest) = strip(value, "label") {
        return Some(Locator::simple(Kind::Label(NameFilter::exact(rest, false))));
    }
    if let Some(rest) = strip(value, "css") {
        return Some(Locator::simple(Kind::Css(rest.to_string())));
    }
    if let Some(rest) = strip(value, "xpath") {
        return Some(Locator::simple(Kind::XPath(rest.to_string())));
    }
    if let Some(rest) = strip(value, "id") {
        return Some(Locator::simple(Kind::Id(rest.to_string())));
    }
    if let Some(rest) = strip(value, "accessibilityId") {
        return Some(Locator::simple(Kind::TestId(rest.to_string())));
    }
    if let Some(rest) = strip(value, "desc") {
        return Some(Locator::simple(Kind::Label(NameFilter::exact(rest, false))));
    }
    if let Some(rest) = strip(value, "class") {
        let role = role_from_android_class(rest, None);
        return if role == Role::Generic {
            None
        } else {
            Some(Locator::simple(Kind::Role(role)))
        };
    }
    if let Some(rest) = strip(value, "type") {
        let role = role_from_ios_type(rest);
        return if role == Role::Generic {
            None
        } else {
            Some(Locator::simple(Kind::Role(role)))
        };
    }
    if strip(value, "predicate").is_some() {
        return None;
    }

    match platform {
        LegacyPlatform::Web => Some(Locator::simple(Kind::Css(value.to_string()))),
        LegacyPlatform::Android | LegacyPlatform::Apple => None,
    }
}

fn strip<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = value.strip_prefix(prefix)?;
    let rest = rest.strip_prefix(':')?;
    (!rest.is_empty()).then_some(rest)
}

fn is_known_prefix(value: &str) -> bool {
    matches!(
        value,
        "textContains"
            | "text"
            | "labelContains"
            | "label"
            | "css"
            | "xpath"
            | "id"
            | "accessibilityId"
            | "desc"
            | "class"
            | "type"
            | "predicate"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::print::print;

    #[test]
    fn maps_web_prefixes_and_bare_css() {
        assert_eq!(
            to_locator("css:#login", LegacyPlatform::Web)
                .map(|locator| print(&locator))
                .as_deref(),
            Some("css=#login")
        );
        assert_eq!(
            to_locator("xpath://button", LegacyPlatform::Web)
                .map(|locator| print(&locator))
                .as_deref(),
            Some("xpath=//button")
        );
        assert_eq!(
            to_locator("text:Sign in", LegacyPlatform::Web)
                .map(|locator| print(&locator))
                .as_deref(),
            Some("text=\"Sign in\"[exact]")
        );
        assert_eq!(
            to_locator("textContains:Sign", LegacyPlatform::Web)
                .map(|locator| print(&locator))
                .as_deref(),
            Some("text=\"Sign\"")
        );
        assert_eq!(
            to_locator("button.primary", LegacyPlatform::Web)
                .map(|locator| print(&locator))
                .as_deref(),
            Some("css=button.primary")
        );
    }

    #[test]
    fn maps_android_prefixes() {
        assert_eq!(
            to_locator("id:com.app:id/ok", LegacyPlatform::Android)
                .map(|locator| print(&locator))
                .as_deref(),
            Some("id=com.app:id/ok")
        );
        assert_eq!(
            to_locator("desc:Play", LegacyPlatform::Android)
                .map(|locator| print(&locator))
                .as_deref(),
            Some("label=\"Play\"[exact]")
        );
        assert_eq!(
            to_locator("class:android.widget.Button", LegacyPlatform::Android)
                .map(|locator| print(&locator))
                .as_deref(),
            Some("role=button")
        );
        assert!(to_locator("button", LegacyPlatform::Android).is_none());
    }

    #[test]
    fn maps_apple_prefixes_and_rejects_predicates() {
        assert_eq!(
            to_locator("accessibilityId:login", LegacyPlatform::Apple)
                .map(|locator| print(&locator))
                .as_deref(),
            Some("testid=login")
        );
        assert_eq!(
            to_locator("labelContains:Sign", LegacyPlatform::Apple)
                .map(|locator| print(&locator))
                .as_deref(),
            Some("label=\"Sign\"")
        );
        assert_eq!(
            to_locator("type:button", LegacyPlatform::Apple)
                .map(|locator| print(&locator))
                .as_deref(),
            Some("role=button")
        );
        assert!(to_locator("predicate:name == 'x'", LegacyPlatform::Apple).is_none());
        assert!(to_locator("login", LegacyPlatform::Apple).is_none());
    }

    #[test]
    fn passes_locator_expressions_through() {
        assert_eq!(
            to_locator("role=button[name=\"Save\"]", LegacyPlatform::Web)
                .map(|locator| print(&locator))
                .as_deref(),
            Some("role=button[name=\"Save\"]")
        );
    }

    #[test]
    fn absent_markers_are_none() {
        for marker in ["", "-", "~", "none", "null", "  "] {
            assert!(to_locator(marker, LegacyPlatform::Web).is_none());
        }
    }
}
