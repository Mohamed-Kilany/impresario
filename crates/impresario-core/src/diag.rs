//! The diagnostic catalog: stable codes, severities and miette rendering.
//!
//! Every user-facing error carries a stable code from this catalog so that
//! `docs/ERRORS.md` and tests can reference it (section 5.7 of the spec).

use std::fmt;

use miette::Diagnostic as MietteDiagnostic;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::span::Span;

/// Severity of a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Stable diagnostic codes (spec sections 4, 5.7 and 6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Code {
    // configuration
    CfgUnknownKey,
    CfgMissingField,
    CfgInvalidValue,
    CfgDuplicateDevice,
    CfgBadEndpoint,
    CfgDuplicateApp,
    // app binding (planner)
    AppUnknown,
    AppPlatformMissing,
    AppPackageMissing,
    AppConflict,
    // DSL
    DslTabIndentation,
    DslInconsistentIndent,
    DslUnterminatedString,
    DslUnknownKeyword,
    DslMissingHeader,
    DslDuplicateScene,
    DslDuplicatePiece,
    DslUnknownScene,
    DslUnknownPiece,
    DslUnknownAction,
    DslInvalidSelector,
    DslMissingSelectorForPlatform,
    DslUnsupportedActionOnPlatform,
    DslUndefinedVariable,
    DslInvalidDuration,
    DslDuplicateActStage,
    DslSceneNotApplicable,
    DslUnknownKeyName,
    DslLintFixedWait,
    DslLintNaming,
    DslLintSceneFileMismatch,
    DslTimeoutNotAllowed,
    // theatrical DSL (TASK-2 section 8)
    DslDuplicateStage,
    DslUnknownStage,
    DslInvalidStageArguments,
    DslDuplicateTheatre,
    DslStageCycle,
    DslDuplicateParameter,
    DslUnknownParameter,
    DslMissingStageArgument,
    DslDuplicateExampleColumn,
    DslExampleWidth,
    DslUnknownExampleColumn,
    DslStageDepth,
    DslLintUnusedScene,
    DslLintUnusedPiece,
    DslLintUnusedStage,
    DslLintUnusedTheatre,
    DslLintUnusedVar,
    DslLintUnusedExampleColumn,
    DslLintShadowedVar,
    DslLintActPlatformCoverage,
    DslLintRequiredParamNeverSupplied,
}

impl Code {
    /// The stable string form, e.g. `IMP-DSL-011`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Code::CfgUnknownKey => "IMP-CFG-001",
            Code::CfgMissingField => "IMP-CFG-002",
            Code::CfgInvalidValue => "IMP-CFG-003",
            Code::CfgDuplicateDevice => "IMP-CFG-004",
            Code::CfgBadEndpoint => "IMP-CFG-005",
            Code::CfgDuplicateApp => "IMP-CFG-006",
            Code::AppUnknown => "IMP-APP-001",
            Code::AppPlatformMissing => "IMP-APP-002",
            Code::AppPackageMissing => "IMP-APP-003",
            Code::AppConflict => "IMP-APP-004",
            Code::DslTabIndentation => "IMP-DSL-001",
            Code::DslInconsistentIndent => "IMP-DSL-002",
            Code::DslUnterminatedString => "IMP-DSL-003",
            Code::DslUnknownKeyword => "IMP-DSL-004",
            Code::DslMissingHeader => "IMP-DSL-005",
            Code::DslDuplicateScene => "IMP-DSL-006",
            Code::DslDuplicatePiece => "IMP-DSL-007",
            Code::DslUnknownScene => "IMP-DSL-008",
            Code::DslUnknownPiece => "IMP-DSL-009",
            Code::DslUnknownAction => "IMP-DSL-010",
            Code::DslInvalidSelector => "IMP-DSL-011",
            Code::DslMissingSelectorForPlatform => "IMP-DSL-012",
            Code::DslUnsupportedActionOnPlatform => "IMP-DSL-013",
            Code::DslUndefinedVariable => "IMP-DSL-014",
            Code::DslInvalidDuration => "IMP-DSL-015",
            Code::DslDuplicateActStage => "IMP-DSL-016",
            Code::DslSceneNotApplicable => "IMP-DSL-017",
            Code::DslUnknownKeyName => "IMP-DSL-018",
            Code::DslLintFixedWait => "IMP-DSL-019",
            Code::DslLintNaming => "IMP-DSL-020",
            Code::DslLintSceneFileMismatch => "IMP-DSL-021",
            Code::DslTimeoutNotAllowed => "IMP-DSL-022",
            Code::DslDuplicateStage => "IMP-DSL-030",
            Code::DslUnknownStage => "IMP-DSL-031",
            Code::DslInvalidStageArguments => "IMP-DSL-032",
            Code::DslDuplicateTheatre => "IMP-DSL-033",
            Code::DslStageCycle => "IMP-DSL-034",
            Code::DslDuplicateParameter => "IMP-DSL-035",
            Code::DslUnknownParameter => "IMP-DSL-036",
            Code::DslMissingStageArgument => "IMP-DSL-037",
            Code::DslDuplicateExampleColumn => "IMP-DSL-038",
            Code::DslExampleWidth => "IMP-DSL-039",
            Code::DslUnknownExampleColumn => "IMP-DSL-040",
            Code::DslStageDepth => "IMP-DSL-041",
            Code::DslLintUnusedScene => "IMP-DSL-042",
            Code::DslLintUnusedPiece => "IMP-DSL-043",
            Code::DslLintUnusedStage => "IMP-DSL-044",
            Code::DslLintUnusedTheatre => "IMP-DSL-045",
            Code::DslLintUnusedVar => "IMP-DSL-046",
            Code::DslLintUnusedExampleColumn => "IMP-DSL-047",
            Code::DslLintShadowedVar => "IMP-DSL-048",
            Code::DslLintActPlatformCoverage => "IMP-DSL-049",
            Code::DslLintRequiredParamNeverSupplied => "IMP-DSL-050",
        }
    }

    /// Default severity of the code (lint codes are warnings).
    #[must_use]
    pub fn default_severity(self) -> Severity {
        match self {
            Code::DslLintFixedWait
            | Code::DslLintNaming
            | Code::DslLintSceneFileMismatch
            | Code::DslLintUnusedScene
            | Code::DslLintUnusedPiece
            | Code::DslLintUnusedStage
            | Code::DslLintUnusedTheatre
            | Code::DslLintUnusedVar
            | Code::DslLintUnusedExampleColumn
            | Code::DslLintShadowedVar
            | Code::DslLintActPlatformCoverage
            | Code::DslLintRequiredParamNeverSupplied => Severity::Warning,
            _ => Severity::Error,
        }
    }
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One diagnostic: stable code, message, optional span, optional hint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: Code,
    pub severity: Severity,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}

impl Diagnostic {
    #[must_use]
    pub fn error(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            severity: code.default_severity(),
            message: message.into(),
            span: None,
            help: None,
        }
    }

    #[must_use]
    pub fn warning(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            severity: Severity::Warning,
            message: message.into(),
            span: None,
            help: None,
        }
    }

    #[must_use]
    pub fn with_span(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }

    #[must_use]
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    #[must_use]
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// One-line rendering used in summaries: `IMP-DSL-009 file:line:col: msg`.
    #[must_use]
    pub fn one_line(&self) -> String {
        match &self.span {
            Some(span) => format!("{} {}: {}", self.code, span, self.message),
            None => format!("{}: {}", self.code, self.message),
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for Diagnostic {}

impl MietteDiagnostic for Diagnostic {
    fn code<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        Some(Box::new(self.code.as_str()))
    }

    fn severity(&self) -> Option<miette::Severity> {
        Some(match self.severity {
            Severity::Error => miette::Severity::Error,
            Severity::Warning => miette::Severity::Warning,
        })
    }

    fn help<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        self.help
            .as_ref()
            .map(|h| Box::new(h.clone()) as Box<dyn fmt::Display>)
    }

    fn source_code(&self) -> Option<&dyn miette::SourceCode> {
        None
    }

    // Byte-offset labels need the file contents, which core does not hold.
    // The CLI (miette boundary, spec section 14) attaches source + offsets.
}

/// A batch of diagnostics collected from one operation (parse, validate, ...).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Diagnostics(pub Vec<Diagnostic>);

impl Diagnostics {
    #[must_use]
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn push(&mut self, diagnostic: Diagnostic) {
        self.0.push(diagnostic);
    }

    pub fn extend(&mut self, other: impl IntoIterator<Item = Diagnostic>) {
        self.0.extend(other);
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.0.iter().any(Diagnostic::is_error)
    }

    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.0.iter().filter(|d| d.is_error())
    }

    pub fn warnings(&self) -> impl Iterator<Item = &Diagnostic> {
        self.0.iter().filter(|d| !d.is_error())
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Diagnostic> {
        self.0.iter()
    }

    #[must_use]
    pub fn into_vec(self) -> Vec<Diagnostic> {
        self.0
    }
}

impl IntoIterator for Diagnostics {
    type Item = Diagnostic;
    type IntoIter = std::vec::IntoIter<Diagnostic>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a Diagnostics {
    type Item = &'a Diagnostic;
    type IntoIter = std::slice::Iter<'a, Diagnostic>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

/// Levenshtein distance between two strings (in characters).
#[must_use]
pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        if let Some(first) = cur.first_mut() {
            *first = i;
        }
        for j in 1..=b.len() {
            let cost = if a.get(i - 1) == b.get(j - 1) { 0 } else { 1 };
            let deletion = prev.get(j).copied().unwrap_or(usize::MAX) + 1;
            let insertion = cur.get(j - 1).copied().unwrap_or(usize::MAX) + 1;
            let substitution = prev.get(j - 1).copied().unwrap_or(usize::MAX) + cost;
            if let Some(slot) = cur.get_mut(j) {
                *slot = deletion.min(insertion).min(substitution);
            }
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev.get(b.len()).copied().unwrap_or(usize::MAX)
}

/// A "did you mean 'x'?" hint for a near miss among `candidates`.
///
/// Case-only mismatches always match; otherwise a prefix relation or an edit
/// distance of at most 2 wins.
#[must_use]
pub fn suggest(word: &str, candidates: &[&str]) -> Option<String> {
    let lower = word.to_ascii_lowercase();
    candidates
        .iter()
        .copied()
        .find(|candidate| candidate.to_ascii_lowercase() == lower && *candidate != word)
        .or_else(|| {
            candidates.iter().copied().find(|candidate| {
                candidate.starts_with(&lower)
                    || lower.starts_with(&candidate.to_ascii_lowercase())
                    || edit_distance(&candidate.to_ascii_lowercase(), &lower) <= 2
            })
        })
        .map(|candidate| format!("did you mean '{candidate}'?"))
}

/// Errors that can be turned into diagnostics by callers with file context.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("yaml error in {path}: {source}")]
    Yaml {
        path: String,
        #[source]
        source: serde_yaml_ng::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        assert_eq!(Code::CfgUnknownKey.as_str(), "IMP-CFG-001");
        assert_eq!(Code::DslInvalidSelector.as_str(), "IMP-DSL-011");
        assert_eq!(Code::DslTimeoutNotAllowed.as_str(), "IMP-DSL-022");
    }

    #[test]
    fn lint_codes_are_warnings() {
        assert_eq!(Code::DslLintNaming.default_severity(), Severity::Warning);
        assert_eq!(Code::DslUnknownScene.default_severity(), Severity::Error);
    }

    #[test]
    fn one_line_includes_position() {
        let diag = Diagnostic::error(Code::DslUnknownPiece, "unknown piece Foo")
            .with_span(Span::new("plays/a.play", 4, 9, 3));
        assert_eq!(
            diag.one_line(),
            "IMP-DSL-009 plays/a.play:4:9: unknown piece Foo"
        );
    }

    #[test]
    fn diagnostics_report_error_state() {
        let mut diags = Diagnostics::new();
        diags.push(Diagnostic::warning(Code::DslLintNaming, "naming"));
        assert!(!diags.has_errors());
        diags.push(Diagnostic::error(Code::DslUnknownScene, "nope"));
        assert!(diags.has_errors());
        assert_eq!(diags.errors().count(), 1);
        assert_eq!(diags.warnings().count(), 1);
    }
}
