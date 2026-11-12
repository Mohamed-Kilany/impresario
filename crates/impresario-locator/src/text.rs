//! Name/text matching rules shared by every locator kind.

use std::sync::Arc;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::parse::LocatorError;

/// How a text/name value is compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    /// Substring match after whitespace normalization (the default).
    Contains,
    /// Whole-value match after whitespace normalization.
    Exact,
    /// Regular expression over the whitespace-normalized value.
    Regex,
}

/// One text/name filter with a pre-compiled regular expression when needed.
#[derive(Debug, Clone)]
pub struct NameFilter {
    value: String,
    mode: MatchMode,
    ignore_case: bool,
    regex: Option<Arc<Regex>>,
}

impl PartialEq for NameFilter {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
            && self.mode == other.mode
            && self.ignore_case == other.ignore_case
    }
}

impl Eq for NameFilter {}

impl Serialize for NameFilter {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct as _;
        let mut state = serializer.serialize_struct("NameFilter", 3)?;
        state.serialize_field("value", &self.value)?;
        state.serialize_field("mode", &self.mode)?;
        state.serialize_field("ignore_case", &self.ignore_case)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for NameFilter {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Repr {
            value: String,
            mode: MatchMode,
            ignore_case: bool,
        }

        let repr = Repr::deserialize(deserializer)?;
        match repr.mode {
            MatchMode::Contains => Ok(Self::contains(repr.value, repr.ignore_case)),
            MatchMode::Exact => Ok(Self::exact(repr.value, repr.ignore_case)),
            MatchMode::Regex => {
                Self::regex(repr.value, repr.ignore_case).map_err(serde::de::Error::custom)
            }
        }
    }
}

impl NameFilter {
    /// A substring filter.
    pub fn contains(value: impl Into<String>, ignore_case: bool) -> Self {
        Self {
            value: normalize_whitespace(&value.into()),
            mode: MatchMode::Contains,
            ignore_case,
            regex: None,
        }
    }

    /// A whole-value filter.
    pub fn exact(value: impl Into<String>, ignore_case: bool) -> Self {
        Self {
            value: normalize_whitespace(&value.into()),
            mode: MatchMode::Exact,
            ignore_case,
            regex: None,
        }
    }

    /// A regular-expression filter. The pattern is compiled once.
    pub fn regex(pattern: impl Into<String>, ignore_case: bool) -> Result<Self, LocatorError> {
        let value = pattern.into();
        let compiled = if ignore_case {
            format!("(?i:{value})")
        } else {
            value.clone()
        };
        let regex =
            Regex::new(&compiled).map_err(|error| LocatorError::InvalidRegex(error.to_string()))?;
        Ok(Self {
            value,
            mode: MatchMode::Regex,
            ignore_case,
            regex: Some(Arc::new(regex)),
        })
    }

    /// The original value (without regex delimiters).
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }

    /// The matching mode.
    #[must_use]
    pub fn mode(&self) -> MatchMode {
        self.mode
    }

    /// Whether matching is case-insensitive.
    #[must_use]
    pub fn ignore_case(&self) -> bool {
        self.ignore_case
    }

    /// Whether `candidate` matches this filter.
    #[must_use]
    pub fn matches(&self, candidate: &str) -> bool {
        let normalized = normalize_whitespace(candidate);
        match self.mode {
            MatchMode::Regex => self
                .regex
                .as_ref()
                .is_some_and(|regex| regex.is_match(&normalized)),
            MatchMode::Exact => {
                if self.ignore_case {
                    normalized.to_lowercase() == self.value.to_lowercase()
                } else {
                    normalized == self.value
                }
            }
            MatchMode::Contains => {
                if self.ignore_case {
                    normalized
                        .to_lowercase()
                        .contains(&self.value.to_lowercase())
                } else {
                    normalized.contains(&self.value)
                }
            }
        }
    }

    /// Change the mode while keeping the value; regex patterns are
    /// recompiled when switching to regex.
    pub fn with_mode(&self, mode: MatchMode) -> Result<Self, LocatorError> {
        match mode {
            MatchMode::Regex => Self::regex(self.value.clone(), self.ignore_case),
            MatchMode::Exact => Ok(Self::exact(self.value.clone(), self.ignore_case)),
            MatchMode::Contains => Ok(Self::contains(self.value.clone(), self.ignore_case)),
        }
    }

    /// Change case sensitivity.
    pub fn with_ignore_case(&self, ignore_case: bool) -> Result<Self, LocatorError> {
        match self.mode {
            MatchMode::Regex => Self::regex(self.value.clone(), ignore_case),
            MatchMode::Exact => Ok(Self::exact(self.value.clone(), ignore_case)),
            MatchMode::Contains => Ok(Self::contains(self.value.clone(), ignore_case)),
        }
    }
}

/// Trim and collapse every whitespace run to one space.
#[must_use]
pub fn normalize_whitespace(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut pending_space = false;
    for character in value.trim().chars() {
        if character.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space && !out.is_empty() {
            out.push(' ');
        }
        pending_space = false;
        out.push(character);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_is_normalized() {
        assert_eq!(normalize_whitespace("  Sign \n  in\t"), "Sign in");
        assert_eq!(normalize_whitespace(""), "");
    }

    #[test]
    fn contains_exact_and_regex_modes_work() {
        let contains = NameFilter::contains("Sign in", false);
        assert!(contains.matches("Please Sign in now"));
        assert!(!contains.matches("sign in"));

        let exact = NameFilter::exact("Sign in", true);
        assert!(exact.matches("  SIGN   IN "));
        assert!(!exact.matches("Please sign in"));

        let regex = NameFilter::regex("^Hi, .+$", false).expect("regex");
        assert!(regex.matches("Hi, Ada"));
        assert!(!regex.matches("Hi Ada"));
    }

    #[test]
    fn invalid_regex_is_reported() {
        assert!(NameFilter::regex("(", false).is_err());
    }

    #[test]
    fn mode_changes_preserve_the_value() {
        let filter = NameFilter::contains("A B", false);
        let exact = filter.with_mode(MatchMode::Exact).expect("exact");
        assert_eq!(exact.mode(), MatchMode::Exact);
        assert_eq!(exact.value(), "A B");
        assert!(exact.matches("A B"));
        assert!(!exact.matches("A B C"));
    }

    #[test]
    fn name_filter_round_trips_through_serde() {
        let filter = NameFilter::regex("^Hi, .+$", true).expect("regex");
        let text = serde_json::to_string(&filter).expect("serialize");
        let restored: NameFilter = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(restored, filter);
        assert_eq!(restored.mode(), MatchMode::Regex);
        assert!(restored.matches("hi, ada"));

        let invalid = r#"{"value":"(","mode":"regex","ignore_case":false}"#;
        assert!(serde_json::from_str::<NameFilter>(invalid).is_err());
    }
}
