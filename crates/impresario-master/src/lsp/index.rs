//! The workspace index of the language server (spec section 13):
//! `SceneName → SceneInfo { uri, name_span, platforms, pieces }` plus the tag
//! vocabulary used by `Tags:` completions.

use std::collections::{BTreeMap, BTreeSet};

use impresario_core::ast::{PieceSelector, PlayFile, SceneFile, TheatreFile};
use impresario_core::platforms::ScenePlatforms;
use impresario_core::span::Span;
use tower_lsp_server::ls_types::Uri;

/// Everything the LSP knows about one scene file.
#[derive(Debug, Clone)]
pub struct SceneInfo {
    /// File URI of the `.scene` file.
    pub uri: Uri,
    /// Span of `Scene: <name>`.
    pub name_span: Span,
    /// `Platform: ...`
    pub platforms: ScenePlatforms,
    pub pieces: Vec<PieceInfo>,
}

impl SceneInfo {
    /// `cross-platform` or the listed platforms — the completion detail.
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
}

/// One piece of a scene, as far as navigation and completion need it.
#[derive(Debug, Clone)]
pub struct PieceInfo {
    pub name: String,
    pub name_span: Span,
    pub selectors: Vec<PieceSelector>,
    /// `index=N` (0-based, default 0).
    pub index: u32,
}

impl PieceInfo {
    /// `android="id:a", web="#a"` — the completion `detail` and hover rows.
    #[must_use]
    pub fn selectors_summary(&self) -> String {
        selectors_summary(&self.selectors)
    }
}

/// One stage of a theatre, as far as navigation and completion need it.
#[derive(Debug, Clone)]
pub struct StageInfo {
    /// File URI of the `.theatre` file.
    pub uri: Uri,
    pub name_span: Span,
    pub theatre: String,
    /// `Stage: Name(param, optional = 'x')`.
    pub signature: String,
    pub description: Option<String>,
    /// `(name, default)` pairs in declaration order.
    pub params: Vec<(String, Option<String>)>,
}

/// `android="id:a", web="#a"` for a list of selectors.
#[must_use]
pub fn selectors_summary(selectors: &[PieceSelector]) -> String {
    selectors
        .iter()
        .map(|selector| format!("{}=\"{}\"", selector.key.as_str(), selector.raw))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The scene index built from parsed `.scene` files (with overlay content
/// taking precedence over disk).
#[derive(Debug, Clone, Default)]
pub struct Index {
    /// Sorted by scene name.
    pub scenes: BTreeMap<String, SceneInfo>,
    /// Sorted by stage name (globally unique).
    pub stages: BTreeMap<String, StageInfo>,
    /// Every theatre name (sorted).
    pub theatres: BTreeSet<String>,
    /// Every tag used by any act of any play (sorted).
    pub tags: BTreeSet<String>,
}

impl Index {
    /// Build the index from the parsed workspace.
    #[must_use]
    pub fn build(scenes: &[SceneFile], theatres: &[TheatreFile], plays: &[PlayFile]) -> Self {
        let mut index = Index::default();
        for scene in scenes {
            let Some(uri) = Uri::from_file_path(&scene.path) else {
                tracing::warn!("lsp: no file URI for {}", scene.path.display());
                continue;
            };
            let pieces = scene
                .pieces
                .iter()
                .map(|piece| PieceInfo {
                    name: piece.name.clone(),
                    name_span: piece.name_span.clone(),
                    selectors: piece.selectors.clone(),
                    index: piece.index,
                })
                .collect();
            index.scenes.insert(
                scene.name.clone(),
                SceneInfo {
                    uri,
                    name_span: scene.name_span.clone(),
                    platforms: scene.platforms.clone(),
                    pieces,
                },
            );
        }
        for theatre in theatres {
            let Some(uri) = Uri::from_file_path(&theatre.path) else {
                tracing::warn!("lsp: no file URI for {}", theatre.path.display());
                continue;
            };
            index.theatres.insert(theatre.name.clone());
            for stage in &theatre.stages {
                let params: Vec<(String, Option<String>)> = stage
                    .params
                    .iter()
                    .map(|param| {
                        (
                            param.name.clone(),
                            param.default.as_ref().map(|value| value.raw.clone()),
                        )
                    })
                    .collect();
                let signature = if params.is_empty() {
                    format!("Stage: {}", stage.name)
                } else {
                    format!(
                        "Stage: {}({})",
                        stage.name,
                        params
                            .iter()
                            .map(|(name, default)| match default {
                                Some(default) => format!("{name} = '{default}'"),
                                None => name.clone(),
                            })
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                index.stages.insert(
                    stage.name.clone(),
                    StageInfo {
                        uri: uri.clone(),
                        name_span: stage.name_span.clone(),
                        theatre: theatre.name.clone(),
                        signature,
                        description: stage.description.clone(),
                        params,
                    },
                );
            }
        }
        for play in plays {
            for act in &play.acts {
                for tag in &act.tags {
                    index.tags.insert(tag.name.clone());
                }
            }
        }
        index
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

    /// One stage by name.
    #[must_use]
    pub fn stage(&self, stage: &str) -> Option<&StageInfo> {
        if let Some(info) = self.stages.get(stage) {
            return Some(info);
        }
        for (index, _) in stage.match_indices('.').rev() {
            let (theatre, rest) = stage.split_at(index);
            let Some(name) = rest.get(1..) else {
                continue;
            };
            if let Some(info) = self.stages.get(name) {
                if info.theatre == theatre {
                    return Some(info);
                }
            }
        }
        None
    }
}
