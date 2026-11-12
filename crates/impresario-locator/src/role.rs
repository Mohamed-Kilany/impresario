//! Normalized role vocabulary and the per-platform role maps.
//!
//! The table is deliberately data-driven and total: an unknown platform type
//! maps to [`Role::Generic`], never to an error. User-written `role=...`
//! values are validated separately by [`Role::from_str`].

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// The normalized role vocabulary (a superset of the ARIA roles used by the
/// inspector).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Button,
    Link,
    TextBox,
    SearchBox,
    Checkbox,
    Radio,
    Switch,
    Slider,
    SpinButton,
    ComboBox,
    ListBox,
    Option,
    Menu,
    MenuItem,
    Tab,
    TabList,
    TabPanel,
    List,
    ListItem,
    Grid,
    Row,
    Cell,
    Image,
    Heading,
    Text,
    Dialog,
    Alert,
    ProgressBar,
    ScrollBar,
    Toolbar,
    Navigation,
    Video,
    Generic,
}

impl Role {
    /// Every role, in a stable order.
    pub const ALL: &'static [Role] = &[
        Role::Button,
        Role::Link,
        Role::TextBox,
        Role::SearchBox,
        Role::Checkbox,
        Role::Radio,
        Role::Switch,
        Role::Slider,
        Role::SpinButton,
        Role::ComboBox,
        Role::ListBox,
        Role::Option,
        Role::Menu,
        Role::MenuItem,
        Role::Tab,
        Role::TabList,
        Role::TabPanel,
        Role::List,
        Role::ListItem,
        Role::Grid,
        Role::Row,
        Role::Cell,
        Role::Image,
        Role::Heading,
        Role::Text,
        Role::Dialog,
        Role::Alert,
        Role::ProgressBar,
        Role::ScrollBar,
        Role::Toolbar,
        Role::Navigation,
        Role::Video,
        Role::Generic,
    ];

    /// Canonical lower-case spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Button => "button",
            Role::Link => "link",
            Role::TextBox => "textbox",
            Role::SearchBox => "searchbox",
            Role::Checkbox => "checkbox",
            Role::Radio => "radio",
            Role::Switch => "switch",
            Role::Slider => "slider",
            Role::SpinButton => "spinbutton",
            Role::ComboBox => "combobox",
            Role::ListBox => "listbox",
            Role::Option => "option",
            Role::Menu => "menu",
            Role::MenuItem => "menuitem",
            Role::Tab => "tab",
            Role::TabList => "tablist",
            Role::TabPanel => "tabpanel",
            Role::List => "list",
            Role::ListItem => "listitem",
            Role::Grid => "grid",
            Role::Row => "row",
            Role::Cell => "cell",
            Role::Image => "image",
            Role::Heading => "heading",
            Role::Text => "text",
            Role::Dialog => "dialog",
            Role::Alert => "alert",
            Role::ProgressBar => "progressbar",
            Role::ScrollBar => "scrollbar",
            Role::Toolbar => "toolbar",
            Role::Navigation => "navigation",
            Role::Video => "video",
            Role::Generic => "generic",
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A role name that is not in the normalized vocabulary.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown role '{0}'")]
pub struct RoleParseError(pub String);

impl FromStr for Role {
    type Err = RoleParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let key: String = value
            .trim()
            .chars()
            .filter(|character| !matches!(character, '-' | '_' | ' '))
            .flat_map(char::to_lowercase)
            .collect();
        let role = match key.as_str() {
            "button" => Role::Button,
            "link" => Role::Link,
            "textbox" | "textfield" | "edittext" | "textinput" => Role::TextBox,
            "searchbox" | "searchfield" => Role::SearchBox,
            "checkbox" | "check" => Role::Checkbox,
            "radio" | "radiobutton" => Role::Radio,
            "switch" | "toggle" => Role::Switch,
            "slider" => Role::Slider,
            "spinbutton" | "numberinput" => Role::SpinButton,
            "combobox" | "select" | "dropdown" | "popupbutton" | "spinner" => Role::ComboBox,
            "listbox" => Role::ListBox,
            "option" => Role::Option,
            "menu" => Role::Menu,
            "menuitem" => Role::MenuItem,
            "tab" => Role::Tab,
            "tablist" => Role::TabList,
            "tabpanel" => Role::TabPanel,
            "list" | "table" | "collectionview" | "recyclerview" | "listview" => Role::List,
            "listitem" | "item" => Role::ListItem,
            "grid" => Role::Grid,
            "row" | "tablerow" | "outlinerow" => Role::Row,
            "cell" | "tablecell" => Role::Cell,
            "image" | "img" | "icon" => Role::Image,
            "heading" | "header" => Role::Heading,
            "text" | "statictext" | "textview" => Role::Text,
            "dialog" | "sheet" => Role::Dialog,
            "alert" => Role::Alert,
            "progressbar" | "progressindicator" | "activityindicator" => Role::ProgressBar,
            "scrollbar" => Role::ScrollBar,
            "toolbar" => Role::Toolbar,
            "navigation" | "navigationbar" | "nav" => Role::Navigation,
            "video" => Role::Video,
            "generic" | "group" | "other" | "application" | "window" | "scrollview" => {
                Role::Generic
            }
            _ => return Err(RoleParseError(value.trim().to_string())),
        };
        Ok(role)
    }
}

/// Implicit ARIA role for a DOM tag (a practical subset of HTML-AAM).
#[must_use]
pub fn role_from_web_tag(tag: &str, input_type: Option<&str>, explicit: Option<&str>) -> Role {
    if let Some(explicit) = explicit.map(str::trim).filter(|value| !value.is_empty()) {
        return Role::from_str(explicit).unwrap_or(Role::Generic);
    }
    let tag = tag.trim().to_ascii_lowercase();
    match tag.as_str() {
        "button" => Role::Button,
        "a" => Role::Link,
        "textarea" => Role::TextBox,
        "select" => Role::ComboBox,
        "option" => Role::Option,
        "img" => Role::Image,
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => Role::Heading,
        "ul" | "ol" | "menu" => Role::List,
        "li" => Role::ListItem,
        "table" => Role::Grid,
        "tr" => Role::Row,
        "td" | "th" => Role::Cell,
        "dialog" => Role::Dialog,
        "nav" => Role::Navigation,
        "progress" => Role::ProgressBar,
        "video" => Role::Video,
        "p" | "span" | "label" | "strong" | "em" | "b" | "i" => Role::Text,
        "input" => match input_type
            .unwrap_or("text")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "button" | "submit" | "reset" | "image" => Role::Button,
            "checkbox" => Role::Checkbox,
            "radio" => Role::Radio,
            "range" => Role::Slider,
            "number" => Role::SpinButton,
            "search" => Role::SearchBox,
            _ => Role::TextBox,
        },
        _ => Role::Generic,
    }
}

/// Android `AccessibilityNodeInfo` class name / role description.
#[must_use]
pub fn role_from_android_class(class: &str, role_description: Option<&str>) -> Role {
    if let Some(description) = role_description
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if let Ok(role) = Role::from_str(description) {
            return role;
        }
    }
    let class = class.trim().to_ascii_lowercase();
    if class.contains("imagebutton") {
        return Role::Button;
    }
    if class.contains("button") {
        return Role::Button;
    }
    if class.contains("edittext") || class.contains("textfield") {
        return Role::TextBox;
    }
    if class.contains("checkbox") {
        return Role::Checkbox;
    }
    if class.contains("switch") || class.contains("togglebutton") {
        return Role::Switch;
    }
    if class.contains("radiobutton") {
        return Role::Radio;
    }
    if class.contains("seekbar") || class.contains("slider") {
        return Role::Slider;
    }
    if class.contains("spinner") {
        return Role::ComboBox;
    }
    if class.contains("recyclerview") || class.contains("listview") {
        return Role::List;
    }
    if class.contains("imageview") || class.contains("image") {
        return Role::Image;
    }
    if class.contains("textview") {
        return Role::Text;
    }
    if class.contains("progressbar") {
        return Role::ProgressBar;
    }
    if class.contains("tab") {
        return Role::Tab;
    }
    Role::Generic
}

/// Apple `XCUIElementType` name.
#[must_use]
pub fn role_from_ios_type(element_type: &str) -> Role {
    let key = element_type
        .trim()
        .chars()
        .filter(|character| !matches!(character, '-' | '_' | ' '))
        .flat_map(char::to_lowercase)
        .collect::<String>();
    let key = key.strip_prefix("xcuielementtype").unwrap_or(&key);
    match key {
        "button" | "toolbarbutton" | "menubutton" => Role::Button,
        "link" => Role::Link,
        "textfield" | "textview" => Role::TextBox,
        "searchfield" => Role::SearchBox,
        "securetextfield" => Role::TextBox,
        "checkbox" => Role::Checkbox,
        "radiobutton" => Role::Radio,
        "switch" | "toggle" => Role::Switch,
        "slider" => Role::Slider,
        "pickerwheel" => Role::SpinButton,
        "combobox" | "popupbutton" | "picker" => Role::ComboBox,
        "table" | "collectionview" | "outline" => Role::List,
        "cell" | "tablecell" | "outlinerow" => Role::Cell,
        "image" | "icon" => Role::Image,
        "statictext" => Role::Text,
        "navigationbar" => Role::Navigation,
        "tabbar" => Role::TabList,
        "tab" | "tabgroup" => Role::Tab,
        "alert" => Role::Alert,
        "sheet" | "dialog" => Role::Dialog,
        "progressindicator" | "activityindicator" => Role::ProgressBar,
        "scrollbar" => Role::ScrollBar,
        "toolbar" => Role::Toolbar,
        "video" => Role::Video,
        _ => Role::Generic,
    }
}

/// Flutter `SemanticsFlag` names.
#[must_use]
pub fn role_from_flutter_flags(flags: &[&str]) -> Role {
    let has = |name: &str| flags.iter().any(|flag| flag.eq_ignore_ascii_case(name));
    if has("isButton") {
        return Role::Button;
    }
    if has("isTextField") {
        return Role::TextBox;
    }
    if has("isLink") {
        return Role::Link;
    }
    if has("isHeader") {
        return Role::Heading;
    }
    if has("isImage") {
        return Role::Image;
    }
    if has("isSlider") {
        return Role::Slider;
    }
    if has("hasCheckedState") && has("isInMutuallyExclusiveGroup") {
        return Role::Radio;
    }
    if has("hasToggledState") {
        return Role::Switch;
    }
    if has("hasCheckedState") {
        return Role::Checkbox;
    }
    if has("isTextField") {
        return Role::TextBox;
    }
    if has("scrollsAcross") || has("hasImplicitScrolling") {
        return Role::Generic;
    }
    Role::Generic
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_names_round_trip() {
        for role in Role::ALL {
            assert_eq!(Role::from_str(role.as_str()), Ok(*role));
        }
    }

    #[test]
    fn aliases_are_total_and_unknown_input_is_an_error() {
        assert_eq!(Role::from_str("text-box"), Ok(Role::TextBox));
        assert_eq!(
            Role::from_str("XCUIElementTypeButton"),
            Err(RoleParseError("XCUIElementTypeButton".to_string()))
        );
        assert_eq!(Role::from_str("Button"), Ok(Role::Button));
        assert!(Role::from_str("nonsense").is_err());
    }

    #[test]
    fn web_implicit_roles_cover_forms_and_landmarks() {
        assert_eq!(role_from_web_tag("button", None, None), Role::Button);
        assert_eq!(
            role_from_web_tag("input", Some("checkbox"), None),
            Role::Checkbox
        );
        assert_eq!(
            role_from_web_tag("input", Some("search"), None),
            Role::SearchBox
        );
        assert_eq!(role_from_web_tag("a", None, None), Role::Link);
        assert_eq!(role_from_web_tag("h2", None, None), Role::Heading);
        assert_eq!(role_from_web_tag("div", None, Some("button")), Role::Button);
        assert_eq!(role_from_web_tag("div", None, None), Role::Generic);
    }

    #[test]
    fn android_and_apple_maps_are_total() {
        assert_eq!(
            role_from_android_class("android.widget.Button", None),
            Role::Button
        );
        assert_eq!(
            role_from_android_class("android.widget.ImageButton", None),
            Role::Button
        );
        assert_eq!(
            role_from_android_class("android.widget.EditText", None),
            Role::TextBox
        );
        assert_eq!(
            role_from_android_class("androidx.recyclerview.widget.RecyclerView", None),
            Role::List
        );
        assert_eq!(
            role_from_android_class("com.example.Custom", None),
            Role::Generic
        );
        assert_eq!(role_from_ios_type("XCUIElementTypeButton"), Role::Button);
        assert_eq!(
            role_from_ios_type("XCUIElementTypeSecureTextField"),
            Role::TextBox
        );
        assert_eq!(
            role_from_ios_type("XCUIElementTypeNavigationBar"),
            Role::Navigation
        );
        assert_eq!(role_from_ios_type("XCUIElementTypeMystery"), Role::Generic);
    }

    #[test]
    fn flutter_flags_are_ordered_by_specificity() {
        assert_eq!(role_from_flutter_flags(&["isButton"]), Role::Button);
        assert_eq!(
            role_from_flutter_flags(&["hasCheckedState", "isInMutuallyExclusiveGroup"]),
            Role::Radio
        );
        assert_eq!(
            role_from_flutter_flags(&["hasCheckedState"]),
            Role::Checkbox
        );
        assert_eq!(role_from_flutter_flags(&["hasToggledState"]), Role::Switch);
        assert_eq!(role_from_flutter_flags(&[]), Role::Generic);
    }
}
