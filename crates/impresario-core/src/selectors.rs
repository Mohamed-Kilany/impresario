//! Selector grammar per selector key (spec section 5.3).
//!
//! Prefix parsing rule: a prefix is recognised only if the value starts with
//! exactly one of the listed tokens followed by `:`. Anything else is a bare
//! value.

use serde::{Deserialize, Serialize};

use crate::diag::{Code, Diagnostic};
use crate::platforms::Platform;
use crate::span::Span;

/// Wire strategies (names mirror `impresario.v1.SelectorStrategy`; core does
/// not depend on the proto crate, D-16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Strategy {
    Css,
    XPath,
    Text,
    TextContains,
    AndroidId,
    AndroidDesc,
    AndroidClass,
    AccessibilityId,
    Label,
    LabelContains,
    Predicate,
    IosType,
}

impl Strategy {
    /// The DSL prefix, when the strategy is written with one.
    #[must_use]
    pub fn prefix(self) -> &'static str {
        match self {
            Strategy::Css => "css",
            Strategy::XPath => "xpath",
            Strategy::Text => "text",
            Strategy::TextContains => "textContains",
            Strategy::AndroidId => "id",
            Strategy::AndroidDesc => "desc",
            Strategy::AndroidClass => "class",
            Strategy::AccessibilityId => "accessibilityId",
            Strategy::Label => "label",
            Strategy::LabelContains => "labelContains",
            Strategy::Predicate => "predicate",
            Strategy::IosType => "type",
        }
    }

    /// Prefixes valid for a selector key (for completion and diagnostics).
    #[must_use]
    pub fn valid_for(key: Platform) -> &'static [Strategy] {
        match key {
            Platform::Web | Platform::WebOs | Platform::Tizen => &[
                Strategy::Css,
                Strategy::XPath,
                Strategy::Text,
                Strategy::TextContains,
            ],
            Platform::Android | Platform::AndroidTv => &[
                Strategy::AndroidId,
                Strategy::Text,
                Strategy::TextContains,
                Strategy::AndroidDesc,
                Strategy::AndroidClass,
            ],
            Platform::Ios | Platform::Tvos => &[
                Strategy::AccessibilityId,
                Strategy::Label,
                Strategy::LabelContains,
                Strategy::Predicate,
                Strategy::IosType,
            ],
        }
    }
}

/// A resolved selector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selector {
    /// The selector key it was written under.
    pub key: Platform,
    pub strategy: Strategy,
    pub value: String,
    /// 0-based index applied when the selector matches several elements.
    pub index: u32,
}

/// XCUIElement.ElementType names accepted by `type:` (lowerCamel).
pub const IOS_TYPE_NAMES: &[&str] = &[
    "any",
    "other",
    "application",
    "group",
    "window",
    "sheet",
    "drawer",
    "alert",
    "dialog",
    "button",
    "radioButton",
    "radioGroup",
    "checkBox",
    "disclosureTriangle",
    "popUpButton",
    "comboBox",
    "menuButton",
    "toolbarButton",
    "popover",
    "keyboard",
    "key",
    "navigationBar",
    "tabBar",
    "tabGroup",
    "toolbar",
    "statusBar",
    "table",
    "tableRow",
    "tableColumn",
    "outline",
    "outlineRow",
    "browser",
    "collectionView",
    "slider",
    "pageIndicator",
    "progressIndicator",
    "activityIndicator",
    "segmentedControl",
    "picker",
    "pickerWheel",
    "switch",
    "toggle",
    "link",
    "image",
    "icon",
    "searchField",
    "secureTextField",
    "scrollView",
    "staticText",
    "textField",
    "textView",
    "menu",
    "menuItem",
    "menuBar",
    "menuBarItem",
    "map",
    "webView",
    "incrementArrow",
    "decrementArrow",
    "timeline",
    "ratingIndicator",
    "valueIndicator",
    "splitter",
    "refreshIndicator",
    "recyclerView",
    "gridView",
    "touchBar",
    "statusItem",
    "tab",
    "textInput",
    "cell",
    "datePicker",
    "datePickerField",
];

/// Parse a selector value for `key` (strict; spec 5.3) with a 0-based index.
///
/// `span` positions the diagnostic at the selector literal.
pub fn parse(
    key: Platform,
    value: &str,
    index: u32,
    span: Option<Span>,
) -> Result<Selector, Diagnostic> {
    let attach = |mut diag: Diagnostic| -> Diagnostic {
        if let Some(span) = span.clone() {
            diag = diag.with_span(span);
        }
        diag
    };
    let invalid = |reason: String| {
        attach(Diagnostic::error(Code::DslInvalidSelector, reason).with_help(selector_help(key)))
    };

    if value.is_empty() {
        return Err(invalid(format!("empty selector for platform '{key}'")));
    }

    match key {
        Platform::Web | Platform::WebOs | Platform::Tizen => {
            if let Some(rest) = strip(value, Strategy::Css) {
                return non_empty(key, Strategy::Css, rest, index, invalid);
            }
            if let Some(rest) = strip(value, Strategy::XPath) {
                return non_empty(key, Strategy::XPath, rest, index, invalid);
            }
            if let Some(rest) = strip(value, Strategy::Text) {
                return non_empty(key, Strategy::Text, rest, index, invalid);
            }
            if let Some(rest) = strip(value, Strategy::TextContains) {
                return non_empty(key, Strategy::TextContains, rest, index, invalid);
            }
            // Bare value: CSS.
            Ok(Selector {
                key,
                strategy: Strategy::Css,
                value: value.to_string(),
                index,
            })
        }
        Platform::Android | Platform::AndroidTv => {
            if let Some(rest) = strip(value, Strategy::AndroidId) {
                return non_empty(key, Strategy::AndroidId, rest, index, invalid);
            }
            if let Some(rest) = strip(value, Strategy::Text) {
                return non_empty(key, Strategy::Text, rest, index, invalid);
            }
            if let Some(rest) = strip(value, Strategy::TextContains) {
                return non_empty(key, Strategy::TextContains, rest, index, invalid);
            }
            if let Some(rest) = strip(value, Strategy::AndroidDesc) {
                return non_empty(key, Strategy::AndroidDesc, rest, index, invalid);
            }
            if let Some(rest) = strip(value, Strategy::AndroidClass) {
                return non_empty(key, Strategy::AndroidClass, rest, index, invalid);
            }
            Err(invalid(format!(
                "android selectors need a prefix; '{value}' has none"
            )))
        }
        Platform::Ios | Platform::Tvos => {
            if let Some(rest) = strip(value, Strategy::AccessibilityId) {
                return non_empty(key, Strategy::AccessibilityId, rest, index, invalid);
            }
            if let Some(rest) = strip(value, Strategy::Label) {
                return non_empty(key, Strategy::Label, rest, index, invalid);
            }
            if let Some(rest) = strip(value, Strategy::LabelContains) {
                return non_empty(key, Strategy::LabelContains, rest, index, invalid);
            }
            if let Some(rest) = strip(value, Strategy::Predicate) {
                return non_empty(key, Strategy::Predicate, rest, index, invalid);
            }
            if let Some(rest) = strip(value, Strategy::IosType) {
                if !IOS_TYPE_NAMES.contains(&rest) {
                    return Err(invalid(format!("unknown XCUIElement.ElementType '{rest}'")));
                }
                return Ok(Selector {
                    key,
                    strategy: Strategy::IosType,
                    value: rest.to_string(),
                    index,
                });
            }
            Err(invalid(format!(
                "ios selectors need a prefix; '{value}' has none"
            )))
        }
    }
}

fn strip(value: &str, strategy: Strategy) -> Option<&str> {
    let prefix = strategy.prefix();
    let rest = value.strip_prefix(prefix)?;
    rest.strip_prefix(':')
}

fn non_empty(
    key: Platform,
    strategy: Strategy,
    rest: &str,
    index: u32,
    invalid: impl Fn(String) -> Diagnostic,
) -> Result<Selector, Diagnostic> {
    if rest.is_empty() {
        return Err(invalid(format!(
            "selector prefix '{}:' needs a value after it",
            strategy.prefix()
        )));
    }
    Ok(Selector {
        key,
        strategy,
        value: rest.to_string(),
        index,
    })
}

/// Human-readable help for the selector grammar of a key.
#[must_use]
pub fn selector_help(key: Platform) -> String {
    let prefixes: Vec<String> = Strategy::valid_for(key)
        .iter()
        .map(|s| format!("{}:", s.prefix()))
        .collect();
    match key {
        Platform::Web | Platform::WebOs | Platform::Tizen => {
            format!("{key}: bare CSS value, or one of {}", prefixes.join(", "))
        }
        _ => format!("{key}: one of {}", prefixes.join(", ")),
    }
}

/// `textContains` etc. are matched case-sensitively exactly as written.
#[must_use]
pub fn is_valid_key_name(key: &str) -> bool {
    key.parse::<Platform>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_bare_value_is_css() {
        let sel = parse(Platform::Web, "h1.title", 0, None).expect("ok");
        assert_eq!(sel.strategy, Strategy::Css);
        assert_eq!(sel.value, "h1.title");
    }

    #[test]
    fn web_prefixes() {
        assert_eq!(
            parse(Platform::Web, "css:#id", 0, None)
                .expect("ok")
                .strategy,
            Strategy::Css
        );
        assert_eq!(
            parse(Platform::WebOs, "xpath://div/a", 0, None)
                .expect("ok")
                .strategy,
            Strategy::XPath
        );
        assert_eq!(
            parse(Platform::Tizen, "text:Hello", 0, None)
                .expect("ok")
                .strategy,
            Strategy::Text
        );
        assert_eq!(
            parse(Platform::Web, "textContains:ell", 0, None)
                .expect("ok")
                .strategy,
            Strategy::TextContains
        );
    }

    #[test]
    fn bare_value_with_colon_is_css() {
        // Only known prefixes are recognised; "button[type='submit']" must
        // not be misread.
        let sel = parse(Platform::Web, "button[type='submit']", 0, None).expect("ok");
        assert_eq!(sel.strategy, Strategy::Css);
        // Unknown prefix stays CSS (an XPath-ish string is still a CSS string).
        let sel = parse(Platform::Web, "nope:value", 0, None).expect("ok");
        assert_eq!(sel.strategy, Strategy::Css);
        assert_eq!(sel.value, "nope:value");
    }

    #[test]
    fn android_requires_prefix() {
        let err = parse(Platform::Android, "#username", 0, None).expect_err("fail");
        assert_eq!(err.code, Code::DslInvalidSelector);
        assert_eq!(
            parse(Platform::Android, "id:username", 0, None)
                .expect("ok")
                .strategy,
            Strategy::AndroidId
        );
        assert_eq!(
            parse(Platform::AndroidTv, "id:com.pkg:id/name", 0, None)
                .expect("ok")
                .strategy,
            Strategy::AndroidId
        );
        // xpath is NOT supported on android (spec 5.3 note)
        assert!(parse(Platform::Android, "xpath://x", 0, None).is_err());
        assert_eq!(
            parse(Platform::Android, "class:android.widget.TextView", 0, None)
                .expect("ok")
                .strategy,
            Strategy::AndroidClass
        );
        assert_eq!(
            parse(Platform::Android, "desc:play", 0, None)
                .expect("ok")
                .strategy,
            Strategy::AndroidDesc
        );
    }

    #[test]
    fn ios_requires_prefix_and_validates_type() {
        assert!(parse(Platform::Ios, "login", 0, None).is_err());
        assert_eq!(
            parse(Platform::Ios, "accessibilityId:login", 0, None)
                .expect("ok")
                .strategy,
            Strategy::AccessibilityId
        );
        assert_eq!(
            parse(Platform::Tvos, "type:button", 0, None)
                .expect("ok")
                .strategy,
            Strategy::IosType
        );
        assert_eq!(
            parse(Platform::Ios, "type:secureTextField", 0, None)
                .expect("ok")
                .value,
            "secureTextField"
        );
        let err = parse(Platform::Ios, "type:blob", 0, None).expect_err("fail");
        assert_eq!(err.code, Code::DslInvalidSelector);
        assert!(err.message.contains("blob"));
    }

    #[test]
    fn prefix_without_value_is_invalid() {
        for (key, value) in [
            (Platform::Web, "css:"),
            (Platform::Android, "id:"),
            (Platform::Ios, "label:"),
        ] {
            let err = parse(key, value, 0, None).expect_err("fail");
            assert_eq!(err.code, Code::DslInvalidSelector);
        }
    }

    #[test]
    fn empty_selector_is_invalid() {
        for key in Platform::ALL {
            assert!(parse(key, "", 0, None).is_err());
        }
    }

    #[test]
    fn index_is_kept() {
        let sel = parse(Platform::Web, "#x", 2, None).expect("ok");
        assert_eq!(sel.index, 2);
    }

    #[test]
    fn help_mentions_key() {
        assert!(selector_help(Platform::Android).contains("id:"));
        assert!(selector_help(Platform::Ios).contains("predicate:"));
        assert!(selector_help(Platform::Tizen).contains("bare CSS"));
    }
}
