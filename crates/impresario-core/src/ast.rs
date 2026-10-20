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
    /// `Platform: ...` / `Platforms: ...`
    pub platforms: ScenePlatforms,
    pub platforms_span: Span,
    /// Column order of the table form (empty for the legacy `Pieces:` form).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<SceneColumn>,
    pub pieces: Vec<PieceDecl>,
}

/// One column of a tabular scene.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneColumn {
    /// `None` for the `index` column.
    pub platform: Option<Platform>,
    pub name: String,
    pub span: Span,
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
    /// `App: <name>` (TASK-2 section 5); the planner binds it to every device.
    pub app: Option<Named>,
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

/// A `.theatre` file: named, reusable stages (TASK-2 section 8.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TheatreFile {
    pub path: PathBuf,
    /// `Theatre: Name`
    pub name: String,
    pub name_span: Span,
    pub stages: Vec<StageDecl>,
}

impl TheatreFile {
    #[must_use]
    pub fn stage(&self, name: &str) -> Option<&StageDecl> {
        self.stages.iter().find(|stage| stage.name == name)
    }
}

/// `Act: ...` with its tags, platforms and performances.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActDecl {
    pub name: String,
    pub name_span: Span,
    /// `Act: Name(param, ...)` parameters.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<ParamDecl>,
    /// `Tags: a, b`
    pub tags: Vec<Named>,
    /// `Platforms: a, b` (empty = all platforms).
    pub platforms: Vec<Named>,
    /// `Examples:` rows (one execution each).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub examples: Option<ExampleTable>,
    /// New-style `Perform:` lines.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub performs: Vec<PerformDecl>,
    /// Legacy inline `Stage:` blocks (kept for `migrate`/backward compatibility).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stages: Vec<StageDecl>,
}

/// `Stage: Name(param, ...)` declaration (theatre or legacy inline).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageDecl {
    pub name: String,
    pub name_span: Span,
    /// `Stage: Name(param, ...)` parameters.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<ParamDecl>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// `Platforms:` restriction inside the stage.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub platforms: Vec<Named>,
    pub timeout: Option<DslDuration>,
    pub timeout_span: Option<Span>,
    /// Ordered body of the stage: scene blocks and nested `Perform:` calls.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<StageItem>,
    /// Legacy view of the scene blocks (kept for old parsers/tests).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scenes: Vec<SceneBlock>,
}

/// One ordered item in a stage body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "item", rename_all = "snake_case")]
pub enum StageItem {
    Scene(SceneBlock),
    Perform(PerformDecl),
}

/// A stage parameter declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParamDecl {
    pub name: String,
    pub name_span: Span,
    /// `= 'default'` (omitted means required).
    pub default: Option<ParsedString>,
}

/// One `Perform: Stage(arg = 'value', ...)` reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PerformDecl {
    pub stage_name: String,
    pub stage_span: Span,
    pub args: Vec<Argument>,
    /// Span of the whole declaration.
    pub span: Span,
}

/// One named stage argument.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Argument {
    pub name: String,
    pub name_span: Span,
    pub value: ParsedString,
}

/// An `Examples:` table on an act.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExampleTable {
    pub columns: Vec<ExampleColumn>,
    pub rows: Vec<ExampleRow>,
    pub span: Span,
}

/// One example column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExampleColumn {
    pub name: String,
    pub span: Span,
}

/// One example row; `values` is aligned with `ExampleTable::columns` and
/// `None` means an absent cell (`-`, `~`, `none`, `null` or empty).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExampleRow {
    pub values: Vec<Option<String>>,
    pub span: Span,
}

/// A named item with the span of its name (tags, platforms).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Named {
    pub name: String,
    pub span: Span,
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
