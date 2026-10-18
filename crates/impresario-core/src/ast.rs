//! Typed AST for `.scene` and `.play` files, with spans on every node.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::duration::DslDuration;
use crate::keys::RemoteKey;
use crate::platforms::{Platform, ScenePlatforms};
use crate::span::Span;
use crate::strings::ParsedString;

/// A `.scene` file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneFile {
    pub path: PathBuf,
    /// `Scene: Name`
    pub name: String,
    pub name_span: Span,
    /// `Platform: ...`
    pub platforms: ScenePlatforms,
    pub platforms_span: Span,
    pub pieces: Vec<PieceDecl>,
}

impl SceneFile {
    #[must_use]
    pub fn piece(&self, name: &str) -> Option<&PieceDecl> {
        self.pieces.iter().find(|p| p.name == name)
    }
}

/// One `Name: key="selector", ...` line under `Pieces:`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PieceDecl {
    pub name: String,
    pub name_span: Span,
    /// Span of the whole declaration line.
    pub span: Span,
    pub selectors: Vec<PieceSelector>,
    /// `index=N` (0-based, default 0).
    pub index: u32,
}

/// One `key="selector"` pair of a piece line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PieceSelector {
    pub key: Platform,
    /// The selector text as written (without the surrounding quotes).
    pub raw: String,
    /// Span of the quoted literal including quotes.
    pub span: Span,
}

/// A `.play` file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayFile {
    pub path: PathBuf,
    /// `Play: ...`
    pub name: String,
    pub name_span: Span,
    /// `Description: ...`
    pub description: Option<String>,
    pub var_decls: Vec<VarDecl>,
    pub acts: Vec<ActDecl>,
}

impl PlayFile {
    #[must_use]
    pub fn var(&self, name: &str) -> Option<&VarDecl> {
        self.var_decls.iter().find(|v| v.name == name)
    }
}

/// `Var: NAME = 'value'`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VarDecl {
    pub name: String,
    pub name_span: Span,
    pub value: ParsedString,
}

/// `Act: ...` with its tags, platforms and stages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActDecl {
    pub name: String,
    pub name_span: Span,
    /// `Tags: a, b`
    pub tags: Vec<Named>,
    /// `Platforms: a, b` (empty = all platforms).
    pub platforms: Vec<Named>,
    pub stages: Vec<StageDecl>,
}

/// A named item with the span of its name (tags, platforms).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Named {
    pub name: String,
    pub span: Span,
}

/// `Stage: ...` with optional `Timeout:` and one or more scene blocks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageDecl {
    pub name: String,
    pub name_span: Span,
    pub timeout: Option<DslDuration>,
    pub timeout_span: Option<Span>,
    pub scenes: Vec<SceneBlock>,
}

/// `Scene: X` block inside a stage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneBlock {
    pub scene_name: String,
    pub scene_span: Span,
    pub steps: Vec<Step>,
}

/// One step line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// The trimmed source text of the line, as written.
    pub source_text: String,
    /// Span of the whole line content.
    pub span: Span,
    pub timeout: Option<DslDuration>,
    #[serde(flatten)]
    pub kind: StepKind,
}

impl Step {
    /// The `StepKind` (for matching without the surrounding metadata).
    #[must_use]
    pub fn kind(&self) -> &StepKind {
        &self.kind
    }
}

/// Action or check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum StepKind {
    Action(Action),
    Check(Check),
}

/// The `Action:` body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    pub verb: ActionVerb,
    /// Span of the action body (after `Action:`).
    pub span: Span,
}

/// Action verbs with their arguments. `type`/`append`/`clear` are distinct
/// (D-06): `'x' => P` is `Type` (replace).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ActionVerb {
    Type {
        text: ParsedString,
        piece: Ident,
    },
    Append {
        text: ParsedString,
        piece: Ident,
    },
    Clear {
        piece: Ident,
    },
    Click {
        piece: Ident,
    },
    LongPress {
        duration: DslDuration,
        piece: Ident,
    },
    Focus {
        piece: Ident,
    },
    Select {
        option: ParsedString,
        piece: Ident,
    },
    Scroll {
        direction: Direction,
        count: u32,
        piece: Option<Ident>,
    },
    ScrollTo {
        piece: Ident,
    },
    Swipe {
        direction: Direction,
        piece: Option<Ident>,
    },
    Key {
        key: KeyName,
        repeat: u32,
    },
    Back,
    Wait {
        duration: DslDuration,
    },
    Screenshot {
        name: ParsedString,
    },
    App {
        op: AppOp,
    },
}

impl ActionVerb {
    /// The DSL verb name (for diagnostics and support checks).
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            ActionVerb::Type { .. } => "type",
            ActionVerb::Append { .. } => "append",
            ActionVerb::Clear { .. } => "clear",
            ActionVerb::Click { .. } => "click",
            ActionVerb::LongPress { .. } => "longpress",
            ActionVerb::Focus { .. } => "focus",
            ActionVerb::Select { .. } => "select",
            ActionVerb::Scroll { .. } => "scroll",
            ActionVerb::ScrollTo { .. } => "scrollto",
            ActionVerb::Swipe { .. } => "swipe",
            ActionVerb::Key { .. } => "key",
            ActionVerb::Back => "back",
            ActionVerb::Wait { .. } => "wait",
            ActionVerb::Screenshot { .. } => "screenshot",
            ActionVerb::App { .. } => "app",
        }
    }

    /// The piece this verb acts on, when any.
    #[must_use]
    pub fn piece(&self) -> Option<&Ident> {
        match self {
            ActionVerb::Type { piece, .. }
            | ActionVerb::Append { piece, .. }
            | ActionVerb::Clear { piece }
            | ActionVerb::Click { piece }
            | ActionVerb::LongPress { piece, .. }
            | ActionVerb::Focus { piece }
            | ActionVerb::Select { piece, .. }
            | ActionVerb::ScrollTo { piece } => Some(piece),
            ActionVerb::Scroll { piece, .. } | ActionVerb::Swipe { piece, .. } => piece.as_ref(),
            ActionVerb::Key { .. }
            | ActionVerb::Back
            | ActionVerb::Wait { .. }
            | ActionVerb::Screenshot { .. }
            | ActionVerb::App { .. } => None,
        }
    }
}

/// A piece reference with its span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

/// A key name with its span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyName {
    pub key: RemoteKey,
    pub span: Span,
}

/// Scroll/swipe directions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

impl Direction {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Up => "up",
            Direction::Down => "down",
            Direction::Left => "left",
            Direction::Right => "right",
        }
    }

    /// The opposite direction (used by `scrollto`).
    #[must_use]
    pub fn opposite(self) -> Self {
        match self {
            Direction::Up => Direction::Down,
            Direction::Down => Direction::Up,
            Direction::Left => Direction::Right,
            Direction::Right => Direction::Left,
        }
    }
}

/// `app launch | terminate | restart | background`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppOp {
    Launch,
    Terminate,
    Restart,
    Background,
}

impl AppOp {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            AppOp::Launch => "launch",
            AppOp::Terminate => "terminate",
            AppOp::Restart => "restart",
            AppOp::Background => "background",
        }
    }
}

/// The `Check:` body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub piece: Ident,
    pub condition: CheckCondition,
    /// Span of the check body (after `Check:`).
    pub span: Span,
}

/// Check conditions (`Check: P` alone means `is visible`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "check", rename_all = "snake_case")]
pub enum CheckCondition {
    IsVisible,
    IsHidden,
    IsEnabled,
    IsDisabled,
    IsFocused,
    TextEquals { text: ParsedString },
    TextContains { text: ParsedString },
    Count { count: u32 },
}

impl CheckCondition {
    /// DSL spelling for diagnostics and reports.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            CheckCondition::IsVisible => "is visible",
            CheckCondition::IsHidden => "is hidden",
            CheckCondition::IsEnabled => "is enabled",
            CheckCondition::IsDisabled => "is disabled",
            CheckCondition::IsFocused => "is focused",
            CheckCondition::TextEquals { .. } => "text",
            CheckCondition::TextContains { .. } => "textContains",
            CheckCondition::Count { .. } => "count",
        }
    }
}
