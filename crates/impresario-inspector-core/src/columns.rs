//! Scene column resolution for inspector authoring (spec §9.3).
//!
//! The current DSL keys scene columns by platform (plus the optional `index`
//! column). A device therefore resolves to a column by platform, using the
//! same fallback chain as selector resolution (`webos→web`, `tizen→web`,
//! `androidtv→android`, `tvos→ios`). Duplicate platform columns are rejected
//! by the parser, so ambiguity only arises for hand-built/forward-compatible
//! scenes — in that case this module refuses to guess.

use impresario_core::ast::SceneFile;
use impresario_core::platforms::Platform;
use serde::{Deserialize, Serialize};

/// The result of mapping a device platform to a scene column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ColumnResolution {
    /// Exactly one column matched.
    Resolved {
        /// Column key to write in the scene table.
        column: String,
        /// True when the platform matched exactly; false when a fallback
        /// column (`webos`→`web`, ...) was used.
        exact: bool,
    },
    /// More than one column matched; the caller must choose.
    Ambiguous {
        /// Matching column keys, in scene order.
        candidates: Vec<String>,
        /// Human-readable guidance.
        hint: String,
    },
    /// No column matched; offer to add one.
    Missing {
        /// Device platform that had no column.
        platform: Platform,
        /// Columns that do exist (excluding `index`).
        available: Vec<String>,
        /// Column key to add, after confirmation.
        suggested: String,
    },
}

impl ColumnResolution {
    /// The stable MCP error code for a failed resolution.
    #[must_use]
    pub fn code(&self) -> Option<&'static str> {
        match self {
            ColumnResolution::Resolved { .. } => None,
            ColumnResolution::Ambiguous { .. } => Some("AMBIGUOUS_COLUMN"),
            ColumnResolution::Missing { .. } => Some("MISSING_COLUMN"),
        }
    }

    /// The resolved column key, when unique.
    #[must_use]
    pub fn column(&self) -> Option<&str> {
        match self {
            ColumnResolution::Resolved { column, .. } => Some(column),
            ColumnResolution::Ambiguous { .. } | ColumnResolution::Missing { .. } => None,
        }
    }

    /// True when a unique column was found.
    #[must_use]
    pub fn is_resolved(&self) -> bool {
        matches!(self, ColumnResolution::Resolved { .. })
    }

    /// Convert to a structured error for MCP/HTTP callers.
    pub fn into_result(self) -> Result<String, ColumnError> {
        match self {
            ColumnResolution::Resolved { column, .. } => Ok(column),
            ColumnResolution::Ambiguous { candidates, hint } => {
                Err(ColumnError::AmbiguousColumn { candidates, hint })
            }
            ColumnResolution::Missing {
                platform,
                available,
                suggested,
            } => Err(ColumnError::MissingColumn {
                platform,
                available,
                suggested,
            }),
        }
    }
}

/// A structured column-resolution failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ColumnError {
    #[error("AMBIGUOUS_COLUMN: {hint}")]
    AmbiguousColumn {
        candidates: Vec<String>,
        hint: String,
    },
    #[error(
        "MISSING_COLUMN: the scene has no column for '{platform}'; add '{suggested}' (available: {available:?})"
    )]
    MissingColumn {
        platform: Platform,
        available: Vec<String>,
        suggested: String,
    },
}

impl ColumnError {
    /// The stable MCP error code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            ColumnError::AmbiguousColumn { .. } => "AMBIGUOUS_COLUMN",
            ColumnError::MissingColumn { .. } => "MISSING_COLUMN",
        }
    }
}

/// Resolve the scene column for a device platform.
#[must_use]
pub fn resolve_column(scene: &SceneFile, platform: Platform) -> ColumnResolution {
    let columns: Vec<(&Platform, &str)> = scene
        .columns
        .iter()
        .filter_map(|column| {
            column
                .platform
                .as_ref()
                .map(|platform| (platform, column.name.as_str()))
        })
        .collect();

    let exact: Vec<&str> = columns
        .iter()
        .filter(|(candidate, _)| **candidate == platform)
        .map(|(_, name)| *name)
        .collect();
    match exact.len() {
        1 => {
            return ColumnResolution::Resolved {
                column: exact
                    .first()
                    .map_or_else(String::new, |name| (*name).to_string()),
                exact: true,
            };
        }
        count if count > 1 => {
            return ambiguous(platform, exact);
        }
        _ => {}
    }

    for fallback in platform.selector_chain().iter().skip(1) {
        let matches: Vec<&str> = columns
            .iter()
            .filter(|(candidate, _)| *candidate == fallback)
            .map(|(_, name)| *name)
            .collect();
        match matches.len() {
            1 => {
                return ColumnResolution::Resolved {
                    column: matches
                        .first()
                        .map_or_else(String::new, |name| (*name).to_string()),
                    exact: false,
                };
            }
            count if count > 1 => {
                return ambiguous(platform, matches);
            }
            _ => {}
        }
    }

    ColumnResolution::Missing {
        platform,
        available: columns
            .iter()
            .map(|(_, name)| (*name).to_string())
            .collect(),
        suggested: platform.as_str().to_string(),
    }
}

fn ambiguous(platform: Platform, matches: Vec<&str>) -> ColumnResolution {
    let candidates: Vec<String> = matches.iter().map(|name| (*name).to_string()).collect();
    let hint = format!(
        "the scene declares {} columns matching '{platform}'; choose a column explicitly",
        candidates.len()
    );
    ColumnResolution::Ambiguous { candidates, hint }
}

#[cfg(test)]
mod tests {
    use super::*;
    use impresario_core::ast::{SceneColumn, SceneFile};
    use impresario_core::platforms::ScenePlatforms;
    use impresario_core::span::Span;
    use std::path::PathBuf;

    fn scene(columns: &[(Option<Platform>, &str)]) -> SceneFile {
        SceneFile {
            path: PathBuf::from("Test.scene"),
            name: "Test".to_string(),
            name_span: Span::new("Test.scene", 1, 1, 4),
            platforms: ScenePlatforms::CrossPlatform,
            platforms_span: Span::new("Test.scene", 2, 1, 10),
            columns: columns
                .iter()
                .map(|(platform, name)| SceneColumn {
                    platform: *platform,
                    name: (*name).to_string(),
                    span: Span::new("Test.scene", 3, 1, name.len()),
                })
                .collect(),
            pieces: Vec::new(),
        }
    }

    fn standard() -> SceneFile {
        scene(&[
            (None, "index"),
            (Some(Platform::Web), "web"),
            (Some(Platform::Android), "android"),
            (Some(Platform::Ios), "ios"),
        ])
    }

    #[test]
    fn exact_platform_match_is_resolved() {
        let resolution = resolve_column(&standard(), Platform::Android);
        assert_eq!(
            resolution,
            ColumnResolution::Resolved {
                column: "android".to_string(),
                exact: true
            }
        );
        assert!(resolution.is_resolved());
        assert_eq!(resolution.column(), Some("android"));
    }

    #[test]
    fn fallback_chain_maps_tv_platforms_to_their_family() {
        for (platform, column) in [
            (Platform::WebOs, "web"),
            (Platform::Tizen, "web"),
            (Platform::AndroidTv, "android"),
            (Platform::Tvos, "ios"),
        ] {
            assert_eq!(
                resolve_column(&standard(), platform),
                ColumnResolution::Resolved {
                    column: column.to_string(),
                    exact: false
                }
            );
        }
    }

    #[test]
    fn exact_column_wins_over_a_fallback() {
        let scene = scene(&[
            (Some(Platform::Web), "web"),
            (Some(Platform::WebOs), "webos"),
        ]);
        assert_eq!(
            resolve_column(&scene, Platform::WebOs),
            ColumnResolution::Resolved {
                column: "webos".to_string(),
                exact: true
            }
        );
    }

    #[test]
    fn duplicate_platform_columns_are_ambiguous() {
        let scene = scene(&[
            (Some(Platform::Web), "web"),
            (Some(Platform::Web), "web-alt"),
        ]);
        let resolution = resolve_column(&scene, Platform::Web);
        assert_eq!(resolution.code(), Some("AMBIGUOUS_COLUMN"));
        assert_eq!(
            resolution,
            ColumnResolution::Ambiguous {
                candidates: vec!["web".to_string(), "web-alt".to_string()],
                hint: "the scene declares 2 columns matching 'web'; choose a column explicitly"
                    .to_string()
            }
        );
        assert_eq!(
            resolution.into_result(),
            Err(ColumnError::AmbiguousColumn {
                candidates: vec!["web".to_string(), "web-alt".to_string()],
                hint: "the scene declares 2 columns matching 'web'; choose a column explicitly"
                    .to_string()
            })
        );
    }

    #[test]
    fn missing_column_reports_available_and_suggested() {
        let scene = scene(&[(None, "index"), (Some(Platform::Android), "android")]);
        let resolution = resolve_column(&scene, Platform::WebOs);
        assert_eq!(resolution.code(), Some("MISSING_COLUMN"));
        assert_eq!(
            resolution,
            ColumnResolution::Missing {
                platform: Platform::WebOs,
                available: vec!["android".to_string()],
                suggested: "webos".to_string()
            }
        );
        let error = resolution.into_result().expect_err("missing");
        assert_eq!(error.code(), "MISSING_COLUMN");
        assert!(error.to_string().contains("add 'webos'"));
    }

    #[test]
    fn index_column_is_never_a_candidate() {
        let scene = scene(&[(None, "index")]);
        let resolution = resolve_column(&scene, Platform::Web);
        assert_eq!(
            resolution,
            ColumnResolution::Missing {
                platform: Platform::Web,
                available: Vec::new(),
                suggested: "web".to_string()
            }
        );
    }
}
