//! Workspace model and symbol index shared by every editor front end.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use impresario_core::ast::{
    ActDecl, ParamDecl, PerformDecl, PieceDecl, PlayFile, SceneBlock, SceneColumn, SceneFile,
    StageDecl, StageItem, Step, StepKind, TheatreFile, VarDecl,
};
use impresario_core::ast::{ActionVerb, CheckCondition, PieceSelector};
use impresario_core::platforms::{Platform, ScenePlatforms};
use impresario_core::span::Span;
use impresario_core::strings::{ParsedString, Part};
use impresario_core::workspace::Workspace;

/// One source file handed to the language service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    pub path: PathBuf,
    pub text: String,
}

/// A configured device, as far as the language service is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub name: String,
    pub platform: Platform,
}

/// An app from `impresario.apps.config`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInfo {
    pub name: String,
    pub description: Option<String>,
}

/// Everything the editor knows about one scene file.
#[derive(Debug, Clone)]
pub struct SceneInfo {
    pub path: PathBuf,
    pub name: String,
    pub name_span: Span,
    pub platforms: ScenePlatforms,
    pub platforms_span: Span,
    pub columns: Vec<SceneColumn>,
    pub pieces: Vec<PieceInfo>,
}

impl SceneInfo {
    /// `cross-platform` or the comma-separated platform list.
    #[must_use]
    pub fn platforms_summary(&self) -> String {
        match &self.platforms {
            ScenePlatforms::CrossPlatform => "cross-platform".to_string(),
            ScenePlatforms::List(list) => list
                .iter()
                .map(|platform| platform.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        }
    }

    /// Whether the scene applies to `platform`.
    #[must_use]
    pub fn applies(&self, platform: Platform) -> bool {
        self.platforms.applies_to(platform)
    }
}

/// One piece of a scene.
#[derive(Debug, Clone)]
pub struct PieceInfo {
    pub name: String,
    pub name_span: Span,
    pub span: Span,
    pub selectors: Vec<PieceSelector>,
    pub index: u32,
    /// True for a row of the canonical table form.
    pub tabular: bool,
}

impl PieceInfo {
    /// `android="id:a", web="#a"`.
    #[must_use]
    pub fn selectors_summary(&self) -> String {
        selectors_summary(&self.selectors)
    }
}

/// One stage of a theatre.
#[derive(Debug, Clone)]
pub struct StageInfo {
    pub path: PathBuf,
    pub name: String,
    pub name_span: Span,
    pub theatre: String,
    pub signature: String,
    pub description: Option<String>,
    pub params: Vec<ParamInfo>,
}

/// One parameter declaration.
#[derive(Debug, Clone)]
pub struct ParamInfo {
    pub name: String,
    pub name_span: Span,
    pub default: Option<String>,
}

/// One theatre.
#[derive(Debug, Clone)]
pub struct TheatreInfo {
    pub path: PathBuf,
    pub name: String,
    pub name_span: Span,
}

/// One play.
#[derive(Debug, Clone)]
pub struct PlayInfo {
    pub path: PathBuf,
    pub name: String,
    pub name_span: Span,
    pub description: Option<String>,
    pub app: Option<String>,
    pub vars: Vec<VarInfo>,
    pub acts: Vec<ActInfo>,
}

/// One `Var:` declaration.
#[derive(Debug, Clone)]
pub struct VarInfo {
    pub name: String,
    pub name_span: Span,
    pub value_span: Span,
}

/// One Act.
#[derive(Debug, Clone)]
pub struct ActInfo {
    pub name: String,
    pub name_span: Span,
    pub tags: Vec<String>,
    pub platforms: Vec<Platform>,
    pub params: Vec<ParamInfo>,
    pub examples: Vec<ExampleColumnInfo>,
    pub performs: Vec<PerformInfo>,
}

/// One Examples column.
#[derive(Debug, Clone)]
pub struct ExampleColumnInfo {
    pub name: String,
    pub span: Span,
    pub act: String,
}

/// One `Perform:` call.
#[derive(Debug, Clone)]
pub struct PerformInfo {
    pub stage_name: String,
    pub stage_span: Span,
    pub args: Vec<ArgumentInfo>,
    pub span: Span,
}

/// One named Perform argument.
#[derive(Debug, Clone)]
pub struct ArgumentInfo {
    pub name: String,
    pub name_span: Span,
}

/// The identity of a renameable or navigable symbol.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum SymbolKey {
    Play(String),
    Scene(String),
    Piece {
        scene: String,
        name: String,
    },
    Stage(String),
    Theatre(String),
    Act {
        play: String,
        name: String,
    },
    Var {
        play: String,
        name: String,
    },
    Param {
        owner: String,
        name: String,
    },
    ExampleColumn {
        play: String,
        act: String,
        name: String,
    },
    App(String),
}

/// One occurrence of a symbol, either a declaration or a reference.
#[derive(Debug, Clone)]
pub struct Occurrence {
    pub key: SymbolKey,
    pub span: Span,
    pub definition: bool,
}

/// One symbol in the workspace outline.
#[derive(Debug, Clone)]
pub struct WorkspaceSymbol {
    pub key: SymbolKey,
    pub name: String,
    pub detail: Option<String>,
    pub span: Span,
}

/// The workspace index.
#[derive(Debug, Clone, Default)]
pub struct Index {
    pub scenes: BTreeMap<String, SceneInfo>,
    pub stages: BTreeMap<String, StageInfo>,
    pub theatres: BTreeMap<String, TheatreInfo>,
    pub plays: BTreeMap<PathBuf, PlayInfo>,
    pub tags: BTreeSet<String>,
    pub env_names: BTreeSet<String>,
    pub occurrences: Vec<Occurrence>,
    pub workspace_symbols: Vec<WorkspaceSymbol>,
}

/// The scope used when collecting interpolation references.
#[derive(Clone, Copy)]
struct RefContext<'a> {
    play: Option<&'a str>,
    params: Option<&'a [ParamInfo]>,
    param_owner: Option<&'a str>,
    examples: &'a [ExampleColumnInfo],
    allow_examples: bool,
}

impl Index {
    /// Build the index from a parsed workspace.
    #[must_use]
    pub fn build(workspace: &Workspace) -> Self {
        let mut index = Index::default();
        // Scenes and stages must exist before plays are indexed: `Perform`
        // references resolve against the theatre definitions.
        for scene in &workspace.scenes {
            index.index_scene(scene);
        }
        for theatre in &workspace.theatres {
            index.index_theatre(theatre);
        }
        for play in &workspace.plays {
            index.index_play(play);
        }
        index.occurrences.sort_by(|left, right| {
            (
                left.span.file.clone(),
                left.span.line,
                left.span.col,
                left.span.len,
            )
                .cmp(&(
                    right.span.file.clone(),
                    right.span.line,
                    right.span.col,
                    right.span.len,
                ))
        });
        index
    }

    fn index_scene(&mut self, scene: &SceneFile) {
        let path = scene.path.clone();
        let pieces = scene
            .pieces
            .iter()
            .map(|piece| PieceInfo {
                name: piece.name.clone(),
                name_span: piece.name_span.clone(),
                span: piece.span.clone(),
                selectors: piece.selectors.clone(),
                index: piece.index,
                tabular: !scene.columns.is_empty(),
            })
            .collect::<Vec<_>>();
        self.occurrences.push(Occurrence {
            key: SymbolKey::Scene(scene.name.clone()),
            span: scene.name_span.clone(),
            definition: true,
        });
        for piece in &scene.pieces {
            self.occurrences.push(Occurrence {
                key: SymbolKey::Piece {
                    scene: scene.name.clone(),
                    name: piece.name.clone(),
                },
                span: piece.name_span.clone(),
                definition: true,
            });
        }
        self.workspace_symbols.push(WorkspaceSymbol {
            key: SymbolKey::Scene(scene.name.clone()),
            name: scene.name.clone(),
            detail: Some(platforms_summary(&scene.platforms)),
            span: scene.name_span.clone(),
        });
        for piece in &pieces {
            self.workspace_symbols.push(WorkspaceSymbol {
                key: SymbolKey::Piece {
                    scene: scene.name.clone(),
                    name: piece.name.clone(),
                },
                name: format!("{}.{}", scene.name, piece.name),
                detail: Some(piece.selectors_summary()),
                span: piece.name_span.clone(),
            });
        }
        self.scenes.insert(
            scene.name.clone(),
            SceneInfo {
                path,
                name: scene.name.clone(),
                name_span: scene.name_span.clone(),
                platforms: scene.platforms.clone(),
                platforms_span: scene.platforms_span.clone(),
                columns: scene.columns.clone(),
                pieces,
            },
        );
    }

    fn index_theatre(&mut self, theatre: &TheatreFile) {
        self.occurrences.push(Occurrence {
            key: SymbolKey::Theatre(theatre.name.clone()),
            span: theatre.name_span.clone(),
            definition: true,
        });
        self.workspace_symbols.push(WorkspaceSymbol {
            key: SymbolKey::Theatre(theatre.name.clone()),
            name: theatre.name.clone(),
            detail: Some("theatre".to_string()),
            span: theatre.name_span.clone(),
        });
        // Declare every stage first so a stage can perform another stage that
        // is declared later in the same theatre.
        for stage in &theatre.stages {
            let params = stage
                .params
                .iter()
                .map(|param| ParamInfo {
                    name: param.name.clone(),
                    name_span: param.name_span.clone(),
                    default: param.default.as_ref().map(|value| value.raw.clone()),
                })
                .collect::<Vec<_>>();
            let signature = stage_signature(&stage.name, &params);
            self.occurrences.push(Occurrence {
                key: SymbolKey::Stage(stage.name.clone()),
                span: stage.name_span.clone(),
                definition: true,
            });
            for param in &stage.params {
                self.occurrences.push(Occurrence {
                    key: SymbolKey::Param {
                        owner: stage_owner(&stage.name),
                        name: param.name.clone(),
                    },
                    span: param.name_span.clone(),
                    definition: true,
                });
            }
            self.workspace_symbols.push(WorkspaceSymbol {
                key: SymbolKey::Stage(stage.name.clone()),
                name: stage.name.clone(),
                detail: Some(signature.clone()),
                span: stage.name_span.clone(),
            });
            self.stages.insert(
                stage.name.clone(),
                StageInfo {
                    path: theatre.path.clone(),
                    name: stage.name.clone(),
                    name_span: stage.name_span.clone(),
                    theatre: theatre.name.clone(),
                    signature,
                    description: stage.description.clone(),
                    params,
                },
            );
        }
        for stage in &theatre.stages {
            let params = self
                .stages
                .get(&stage.name)
                .map_or_else(Vec::new, |stage| stage.params.clone());
            for param in &stage.params {
                if let Some(default) = &param.default {
                    self.string_refs(
                        default,
                        RefContext {
                            play: None,
                            params: Some(&params),
                            param_owner: Some(&stage_owner(&stage.name)),
                            examples: &[],
                            allow_examples: false,
                        },
                    );
                }
            }
            self.stage_refs(stage, None, &[]);
        }
        self.theatres.insert(
            theatre.name.clone(),
            TheatreInfo {
                path: theatre.path.clone(),
                name: theatre.name.clone(),
                name_span: theatre.name_span.clone(),
            },
        );
    }

    fn index_play(&mut self, play: &PlayFile) {
        let play_key = play.path.display().to_string();
        self.occurrences.push(Occurrence {
            key: SymbolKey::Play(play_key.clone()),
            span: play.name_span.clone(),
            definition: true,
        });
        self.workspace_symbols.push(WorkspaceSymbol {
            key: SymbolKey::Play(play_key.clone()),
            name: play.name.clone(),
            detail: play.description.clone(),
            span: play.name_span.clone(),
        });
        if let Some(app) = &play.app {
            self.occurrences.push(Occurrence {
                key: SymbolKey::App(app.name.clone()),
                span: app.span.clone(),
                definition: false,
            });
        }
        let mut vars = Vec::new();
        for var in &play.var_decls {
            vars.push(VarInfo {
                name: var.name.clone(),
                name_span: var.name_span.clone(),
                value_span: var.value.span.clone(),
            });
            self.occurrences.push(Occurrence {
                key: SymbolKey::Var {
                    play: play_key.clone(),
                    name: var.name.clone(),
                },
                span: var.name_span.clone(),
                definition: true,
            });
            self.string_refs(
                &var.value,
                RefContext {
                    play: Some(&play_key),
                    params: None,
                    param_owner: None,
                    examples: &[],
                    allow_examples: true,
                },
            );
        }
        let mut acts = Vec::new();
        for act in &play.acts {
            for tag in &act.tags {
                self.tags.insert(tag.name.clone());
            }
            let params = act
                .params
                .iter()
                .map(|param| ParamInfo {
                    name: param.name.clone(),
                    name_span: param.name_span.clone(),
                    default: param.default.as_ref().map(|value| value.raw.clone()),
                })
                .collect::<Vec<_>>();
            self.occurrences.push(Occurrence {
                key: SymbolKey::Act {
                    play: play_key.clone(),
                    name: act.name.clone(),
                },
                span: act.name_span.clone(),
                definition: true,
            });
            self.workspace_symbols.push(WorkspaceSymbol {
                key: SymbolKey::Act {
                    play: play_key.clone(),
                    name: act.name.clone(),
                },
                name: format!("{}.{}", play.name, act.name),
                detail: Some("act".to_string()),
                span: act.name_span.clone(),
            });
            for param in &act.params {
                self.occurrences.push(Occurrence {
                    key: SymbolKey::Param {
                        owner: act_owner(&play_key, &act.name),
                        name: param.name.clone(),
                    },
                    span: param.name_span.clone(),
                    definition: true,
                });
                if let Some(default) = &param.default {
                    self.string_refs(
                        default,
                        RefContext {
                            play: Some(&play_key),
                            params: Some(&params),
                            param_owner: Some(&act_owner(&play_key, &act.name)),
                            examples: &[],
                            allow_examples: true,
                        },
                    );
                }
            }
            let examples = act
                .examples
                .as_ref()
                .map(|table| {
                    table
                        .columns
                        .iter()
                        .map(|column| ExampleColumnInfo {
                            name: column.name.clone(),
                            span: column.span.clone(),
                            act: act.name.clone(),
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            for column in &examples {
                self.occurrences.push(Occurrence {
                    key: SymbolKey::ExampleColumn {
                        play: play_key.clone(),
                        act: act.name.clone(),
                        name: column.name.clone(),
                    },
                    span: column.span.clone(),
                    definition: true,
                });
            }
            for perform in &act.performs {
                self.perform_refs(
                    perform,
                    RefContext {
                        play: Some(&play_key),
                        params: Some(&params),
                        param_owner: Some(&act_owner(&play_key, &act.name)),
                        examples: &examples,
                        allow_examples: true,
                    },
                );
            }
            for stage in &act.stages {
                self.stage_refs(stage, Some(&play_key), &examples);
            }
            acts.push(ActInfo {
                name: act.name.clone(),
                name_span: act.name_span.clone(),
                tags: act.tags.iter().map(|tag| tag.name.clone()).collect(),
                platforms: act
                    .platforms
                    .iter()
                    .filter_map(|named| named.name.parse::<Platform>().ok())
                    .collect(),
                params,
                examples,
                performs: act
                    .performs
                    .iter()
                    .map(|perform| PerformInfo {
                        stage_name: perform.stage_name.clone(),
                        stage_span: perform.stage_span.clone(),
                        args: perform
                            .args
                            .iter()
                            .map(|arg| ArgumentInfo {
                                name: arg.name.clone(),
                                name_span: arg.name_span.clone(),
                            })
                            .collect(),
                        span: perform.span.clone(),
                    })
                    .collect(),
            });
        }
        self.plays.insert(
            play.path.clone(),
            PlayInfo {
                path: play.path.clone(),
                name: play.name.clone(),
                name_span: play.name_span.clone(),
                description: play.description.clone(),
                app: play.app.as_ref().map(|app| app.name.clone()),
                vars,
                acts,
            },
        );
    }

    /// References inside one stage body (its own parameters are in scope).
    fn stage_refs(
        &mut self,
        stage: &StageDecl,
        play: Option<&str>,
        examples: &[ExampleColumnInfo],
    ) {
        let params = stage
            .params
            .iter()
            .map(|param| ParamInfo {
                name: param.name.clone(),
                name_span: param.name_span.clone(),
                default: param.default.as_ref().map(|value| value.raw.clone()),
            })
            .collect::<Vec<_>>();
        if stage.items.is_empty() {
            for block in &stage.scenes {
                self.scene_block_refs(
                    block,
                    RefContext {
                        play,
                        params: Some(&params),
                        param_owner: Some(&stage_owner(&stage.name)),
                        examples,
                        allow_examples: true,
                    },
                );
            }
            return;
        }
        for item in &stage.items {
            match item {
                StageItem::Scene(block) => {
                    self.scene_block_refs(
                        block,
                        RefContext {
                            play,
                            params: Some(&params),
                            param_owner: Some(&stage_owner(&stage.name)),
                            examples,
                            allow_examples: true,
                        },
                    );
                }
                StageItem::Perform(perform) => {
                    self.perform_refs(
                        perform,
                        RefContext {
                            play,
                            params: Some(&params),
                            param_owner: Some(&stage_owner(&stage.name)),
                            examples,
                            allow_examples: true,
                        },
                    );
                }
            }
        }
    }

    fn scene_block_refs(&mut self, block: &SceneBlock, context: RefContext<'_>) {
        self.occurrences.push(Occurrence {
            key: SymbolKey::Scene(block.scene_name.clone()),
            span: block.scene_span.clone(),
            definition: false,
        });
        for step in &block.steps {
            self.step_refs(step, &block.scene_name, context);
        }
    }

    fn step_refs(&mut self, step: &Step, scene: &str, context: RefContext<'_>) {
        match &step.kind {
            StepKind::Action(action) => {
                if let Some(piece) = action.verb.piece() {
                    self.occurrences.push(Occurrence {
                        key: SymbolKey::Piece {
                            scene: scene.to_string(),
                            name: piece.name.clone(),
                        },
                        span: piece.span.clone(),
                        definition: false,
                    });
                }
                for string in action_strings(&action.verb) {
                    self.string_refs(string, context);
                }
            }
            StepKind::Check(check) => {
                self.occurrences.push(Occurrence {
                    key: SymbolKey::Piece {
                        scene: scene.to_string(),
                        name: check.piece.name.clone(),
                    },
                    span: check.piece.span.clone(),
                    definition: false,
                });
                if let CheckCondition::TextEquals { text } | CheckCondition::TextContains { text } =
                    &check.condition
                {
                    self.string_refs(text, context);
                }
            }
        }
    }

    fn perform_refs(&mut self, perform: &PerformDecl, context: RefContext<'_>) {
        let target = self.stage(&perform.stage_name).cloned();
        if let Some(stage) = &target {
            let span = if perform.stage_name == stage.name {
                perform.stage_span.clone()
            } else {
                let suffix = perform
                    .stage_name
                    .rsplit_once('.')
                    .map_or(perform.stage_name.as_str(), |(_, suffix)| suffix);
                let offset = perform
                    .stage_name
                    .chars()
                    .count()
                    .saturating_sub(suffix.chars().count());
                Span::new(
                    perform.stage_span.file.clone(),
                    perform.stage_span.line,
                    perform.stage_span.col + offset,
                    suffix.chars().count(),
                )
            };
            self.occurrences.push(Occurrence {
                key: SymbolKey::Stage(stage.name.clone()),
                span,
                definition: false,
            });
            for arg in &perform.args {
                self.occurrences.push(Occurrence {
                    key: SymbolKey::Param {
                        owner: stage_owner(&stage.name),
                        name: arg.name.clone(),
                    },
                    span: arg.name_span.clone(),
                    definition: false,
                });
            }
        } else {
            self.occurrences.push(Occurrence {
                key: SymbolKey::Stage(perform.stage_name.clone()),
                span: perform.stage_span.clone(),
                definition: false,
            });
        }
        // A qualified `Theatre.Stage` reference also names the theatre.
        if let Some(theatre) = self.qualified_theatre(&perform.stage_name, &perform.stage_span) {
            self.occurrences.push(theatre);
        }
        for arg in &perform.args {
            self.string_refs(&arg.value, context);
        }
    }

    fn qualified_theatre(&self, reference: &str, span: &Span) -> Option<Occurrence> {
        if self
            .stage(reference)
            .is_some_and(|stage| stage.name == reference)
        {
            return None;
        }
        for (index, _) in reference.match_indices('.').rev() {
            let (theatre, rest) = reference.split_at(index);
            let name = rest.get(1..)?;
            if self.theatres.contains_key(theatre) && self.stages.contains_key(name) {
                return Some(Occurrence {
                    key: SymbolKey::Theatre(theatre.to_string()),
                    span: Span::new(
                        span.file.clone(),
                        span.line,
                        span.col,
                        theatre.chars().count(),
                    ),
                    definition: false,
                });
            }
        }
        None
    }

    fn string_refs(&mut self, string: &ParsedString, context: RefContext<'_>) {
        for part in &string.parts {
            match part {
                Part::Env { name, .. } => {
                    self.env_names.insert(name.clone());
                }
                Part::Var { name, span } => {
                    self.occurrences.push(Occurrence {
                        key: SymbolKey::Var {
                            play: context.play.unwrap_or("*").to_string(),
                            name: name.clone(),
                        },
                        span: span.clone(),
                        definition: false,
                    });
                }
                Part::Param { name, span } => {
                    let known = context
                        .params
                        .is_some_and(|params| params.iter().any(|param| param.name == *name));
                    let owner = if known {
                        context.param_owner.unwrap_or("*").to_string()
                    } else {
                        "*".to_string()
                    };
                    self.occurrences.push(Occurrence {
                        key: SymbolKey::Param {
                            owner,
                            name: name.clone(),
                        },
                        span: span.clone(),
                        definition: false,
                    });
                }
                Part::Example { name, span } => {
                    if context.allow_examples {
                        let owner = context
                            .examples
                            .iter()
                            .find(|column| column.name == *name)
                            .map_or_else(
                                || ("*".to_string(), "*".to_string()),
                                |column| {
                                    (context.play.unwrap_or("*").to_string(), column.act.clone())
                                },
                            );
                        self.occurrences.push(Occurrence {
                            key: SymbolKey::ExampleColumn {
                                play: owner.0,
                                act: owner.1,
                                name: name.clone(),
                            },
                            span: span.clone(),
                            definition: false,
                        });
                    }
                }
                Part::Literal(_) => {}
            }
        }
    }

    /// One stage, resolving a theatre-qualified name.
    #[must_use]
    pub fn stage(&self, name: &str) -> Option<&StageInfo> {
        if let Some(stage) = self.stages.get(name) {
            return Some(stage);
        }
        for (index, _) in name.match_indices('.').rev() {
            let (theatre, rest) = name.split_at(index);
            let Some(stage_name) = rest.get(1..) else {
                continue;
            };
            if let Some(stage) = self.stages.get(stage_name) {
                if stage.theatre == theatre {
                    return Some(stage);
                }
            }
        }
        None
    }

    /// One piece of one scene.
    #[must_use]
    pub fn piece(&self, scene: &str, piece: &str) -> Option<&PieceInfo> {
        self.scenes
            .get(scene)?
            .pieces
            .iter()
            .find(|candidate| candidate.name == piece)
    }

    /// Resolve a symbol at a position.
    #[must_use]
    pub fn occurrence_at(&self, file: &Path, line: usize, col: usize) -> Option<&Occurrence> {
        let mut candidates = self
            .occurrences
            .iter()
            .filter(|occurrence| {
                occurrence.span.file == file
                    && occurrence.span.line == line
                    && col >= occurrence.span.col
                    && col <= occurrence.span.col.saturating_add(occurrence.span.len)
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|occurrence| occurrence.span.len);
        candidates.first().copied()
    }

    /// Definition locations of a symbol.
    #[must_use]
    pub fn definitions(&self, key: &SymbolKey) -> Vec<Span> {
        self.matching(key)
            .filter(|occurrence| occurrence.definition)
            .map(|occurrence| occurrence.span.clone())
            .collect()
    }

    /// Every occurrence of a symbol, declarations first.
    #[must_use]
    pub fn references(&self, key: &SymbolKey, include_declaration: bool) -> Vec<Span> {
        let mut spans = self
            .matching(key)
            .filter(|occurrence| include_declaration || !occurrence.definition)
            .map(|occurrence| occurrence.span.clone())
            .collect::<Vec<_>>();
        spans.sort_by(|left, right| {
            (left.file.clone(), left.line, left.col).cmp(&(
                right.file.clone(),
                right.line,
                right.col,
            ))
        });
        spans
    }

    /// Occurrences with the same identity, including wildcard scopes.
    pub(crate) fn matching<'a>(
        &'a self,
        key: &'a SymbolKey,
    ) -> impl Iterator<Item = &'a Occurrence> + 'a {
        self.occurrences
            .iter()
            .filter(move |occurrence| same_symbol(&occurrence.key, key))
    }

    /// A human name for a symbol key.
    #[must_use]
    pub fn key_name(&self, key: &SymbolKey) -> String {
        match key {
            SymbolKey::Play(name)
            | SymbolKey::Scene(name)
            | SymbolKey::Stage(name)
            | SymbolKey::Theatre(name)
            | SymbolKey::App(name) => name.clone(),
            SymbolKey::Piece { name, .. } => name.clone(),
            SymbolKey::Act { name, .. } => name.clone(),
            SymbolKey::Var { name, .. }
            | SymbolKey::Param { name, .. }
            | SymbolKey::ExampleColumn { name, .. } => name.clone(),
        }
    }

    /// The kind used for a semantic token.
    #[must_use]
    pub fn key_kind(key: &SymbolKey) -> &'static str {
        match key {
            SymbolKey::Play(_) | SymbolKey::Scene(_) | SymbolKey::Theatre(_) => "namespace",
            SymbolKey::Act { .. } | SymbolKey::Stage(_) => "function",
            SymbolKey::Piece { .. } => "property",
            SymbolKey::Var { .. } | SymbolKey::Param { .. } | SymbolKey::ExampleColumn { .. } => {
                "variable"
            }
            SymbolKey::App(_) => "class",
        }
    }
}

/// The owner key of a stage parameter.
#[must_use]
pub fn stage_owner(stage: &str) -> String {
    format!("stage:{stage}")
}

/// The owner key of an act parameter.
#[must_use]
pub fn act_owner(play: &str, act: &str) -> String {
    format!("act:{play}#{act}")
}

fn same_symbol(left: &SymbolKey, right: &SymbolKey) -> bool {
    match (left, right) {
        (
            SymbolKey::Var {
                play: left_play,
                name: left_name,
            },
            SymbolKey::Var {
                play: right_play,
                name: right_name,
            },
        ) => {
            left_name == right_name
                && (left_play == right_play || left_play == "*" || right_play == "*")
        }
        (
            SymbolKey::ExampleColumn {
                play: left_play,
                act: left_act,
                name: left_name,
            },
            SymbolKey::ExampleColumn {
                play: right_play,
                act: right_act,
                name: right_name,
            },
        ) => {
            left_name == right_name
                && (left_play == right_play || left_play == "*" || right_play == "*")
                && (left_act == right_act || left_act == "*" || right_act == "*")
        }
        (
            SymbolKey::Param {
                owner: left_owner,
                name: left_name,
            },
            SymbolKey::Param {
                owner: right_owner,
                name: right_name,
            },
        ) => {
            left_name == right_name
                && (left_owner == right_owner || left_owner == "*" || right_owner == "*")
        }
        _ => left == right,
    }
}

fn platforms_summary(platforms: &ScenePlatforms) -> String {
    match platforms {
        ScenePlatforms::CrossPlatform => "cross-platform".to_string(),
        ScenePlatforms::List(list) => list
            .iter()
            .map(|platform| platform.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    }
}

fn stage_signature(name: &str, params: &[ParamInfo]) -> String {
    if params.is_empty() {
        return format!("Stage: {name}");
    }
    format!(
        "Stage: {name}({})",
        params
            .iter()
            .map(|param| match &param.default {
                Some(default) => format!("{} = '{default}'", param.name),
                None => param.name.clone(),
            })
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn action_strings(verb: &ActionVerb) -> Vec<&ParsedString> {
    match verb {
        ActionVerb::Type { text, .. } | ActionVerb::Append { text, .. } => vec![text],
        ActionVerb::Select { option, .. } => vec![option],
        ActionVerb::Screenshot { name } => vec![name],
        _ => Vec::new(),
    }
}

/// `android="id:a", web="#a"`.
#[must_use]
pub fn selectors_summary(selectors: &[PieceSelector]) -> String {
    selectors
        .iter()
        .map(|selector| format!("{}=\"{}\"", selector.key.as_str(), selector.raw))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Build the parameter list displayed for a stage.
#[must_use]
pub fn params_of(params: &[ParamDecl]) -> Vec<ParamInfo> {
    params
        .iter()
        .map(|param| ParamInfo {
            name: param.name.clone(),
            name_span: param.name_span.clone(),
            default: param.default.as_ref().map(|value| value.raw.clone()),
        })
        .collect()
}

/// Build the `Perform` view of a declaration.
#[must_use]
pub fn perform_of(perform: &PerformDecl) -> PerformInfo {
    PerformInfo {
        stage_name: perform.stage_name.clone(),
        stage_span: perform.stage_span.clone(),
        args: perform
            .args
            .iter()
            .map(|arg| ArgumentInfo {
                name: arg.name.clone(),
                name_span: arg.name_span.clone(),
            })
            .collect(),
        span: perform.span.clone(),
    }
}

/// Build the var view of a declaration.
#[must_use]
pub fn var_of(var: &VarDecl) -> VarInfo {
    VarInfo {
        name: var.name.clone(),
        name_span: var.name_span.clone(),
        value_span: var.value.span.clone(),
    }
}

/// Whether a piece declaration came from the tabular form.
#[must_use]
pub fn piece_is_tabular(piece: &PieceDecl, scene: &SceneFile) -> bool {
    !scene.columns.is_empty() && piece.span.len > 0 && !scene.columns.is_empty()
}

/// A stage body item span.
#[must_use]
pub fn stage_item_span(item: &StageItem) -> &Span {
    match item {
        StageItem::Scene(block) => &block.scene_span,
        StageItem::Perform(perform) => &perform.span,
    }
}

/// The last span inside a stage.
#[must_use]
pub fn stage_end(stage: &StageDecl) -> &Span {
    stage
        .items
        .last()
        .map(stage_item_span)
        .or_else(|| stage.scenes.last().map(|block| &block.scene_span))
        .unwrap_or(&stage.name_span)
}

/// The last span inside an act.
#[must_use]
pub fn act_end(act: &ActDecl) -> &Span {
    act.performs
        .last()
        .map(|perform| &perform.span)
        .or_else(|| act.stages.last().map(stage_end))
        .unwrap_or(&act.name_span)
}
