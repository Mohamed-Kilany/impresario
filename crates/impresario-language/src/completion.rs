//! Context-sensitive completion for the current DSL (TASK-2 section 9.1).
//!
//! Completion is computed from the line prefix, the nearest enclosing `Scene:`
//! or `Act:` line, and the workspace index. It deliberately does not require
//! the rest of the file to parse, so it keeps working while the user types.

use std::path::Path;

use impresario_core::keys::RemoteKey;
use impresario_core::platforms::Platform;
use impresario_core::support::{self, CheckVerb, Verb};

use crate::service::{CompletionItem, Language};
use crate::text::{Doc, Position};

const VERBS: &[Verb] = &[
    Verb::Click,
    Verb::Type,
    Verb::Append,
    Verb::Clear,
    Verb::LongPress,
    Verb::Focus,
    Verb::Select,
    Verb::Scroll,
    Verb::ScrollTo,
    Verb::Swipe,
    Verb::Back,
    Verb::Key,
    Verb::Wait,
    Verb::Screenshot,
    Verb::AppLaunch,
    Verb::AppTerminate,
    Verb::AppRestart,
    Verb::AppBackground,
];

const KEYWORDS: &[&str] = &[
    "Play:",
    "Theatre:",
    "Scene:",
    "Stage:",
    "Act:",
    "Description:",
    "App:",
    "Var:",
    "Tags:",
    "Platforms:",
    "Timeout:",
    "Examples:",
    "Perform:",
    "Action:",
    "Check:",
];

const DURATIONS: &[&str] = &[
    "100ms", "250ms", "500ms", "1s", "2s", "3s", "5s", "10s", "30s", "1m", "2m", "5m", "10m",
];

impl Language {
    /// Suggestions for a cursor position.
    #[must_use]
    pub fn completion(&self, path: &Path, position: Position) -> Vec<CompletionItem> {
        let Some(text) = self.source(path) else {
            return Vec::new();
        };
        let doc = Doc::new(text);
        let line = doc.line(position.line);
        let prefix = line
            .chars()
            .take(position.col.saturating_sub(1) as usize)
            .collect::<String>();
        let trimmed = prefix.trim_start();
        let is_scene = extension(path) == "scene";
        let is_play = extension(path) == "play";
        let is_theatre = extension(path) == "theatre";

        // Interpolation inside any string.
        if let Some(items) = interpolation_items(self, path, position, &prefix) {
            return items;
        }

        // Canonical table scene and Examples tables.
        if is_scene {
            if let Some(items) = self.table_scene_items(path, &doc, position, &prefix) {
                return items;
            }
        }
        if is_play {
            if let Some(items) = self.examples_items(path, &doc, position, &prefix) {
                return items;
            }
        }

        if let Some(rest) = trimmed.strip_prefix("Perform:") {
            let after = rest.trim_start();
            if let Some((stage_name, arguments)) = after.split_once('(') {
                if !arguments.contains(')') {
                    return self.stage_param_items(path, stage_name.trim());
                }
            }
            return self.stage_items();
        }

        if let Some(rest) = trimmed.strip_prefix("Scene:") {
            if is_scene {
                return Vec::new();
            }
            let _ = rest;
            return self.scene_items();
        }

        if let Some(rest) = trimmed.strip_prefix("Action:") {
            if rest.contains("=>") {
                return enclosing_scene(&doc, position.line, position.col)
                    .map_or_else(Vec::new, |scene| self.piece_items(&scene));
            }
            let after = rest.trim_start();
            if let Some(partial) = after.strip_prefix("key ") {
                if !partial.contains(char::is_whitespace) && !partial.contains('\'') {
                    return self.key_items();
                }
            }
            if after.starts_with("wait ") || after.starts_with("longpress ") {
                return duration_items();
            }
            if after.starts_with("app ") {
                return app_op_items();
            }
            if after.starts_with("scroll ") || after.starts_with("swipe ") {
                return direction_items();
            }
            return verb_items();
        }

        if let Some(rest) = trimmed.strip_prefix("Check:") {
            let after = rest.trim_start();
            let words = after.split_whitespace().count();
            let trailing = after.chars().last().is_some_and(char::is_whitespace);
            if words >= 2 || (words == 1 && trailing) {
                return condition_items();
            }
            return enclosing_scene(&doc, position.line, position.col)
                .map_or_else(Vec::new, |scene| self.piece_items(&scene));
        }

        if trimmed.starts_with("Tags:") {
            return self.tag_items();
        }
        if trimmed.starts_with("Platforms:") {
            return self.platform_items(is_scene);
        }
        if trimmed.starts_with("App:") && (is_play || is_theatre) {
            return self.app_items();
        }
        if trimmed.starts_with("Timeout:")
            || trimmed.starts_with("Description:")
            || trimmed.starts_with("Var:")
        {
            return Vec::new();
        }
        if is_theatre && trimmed.starts_with("Theatre:") {
            return Vec::new();
        }
        if is_play && trimmed.starts_with("Play:") {
            return Vec::new();
        }

        // At the beginning of a line offer the keywords allowed in this file.
        if trimmed.is_empty() || KEYWORDS.iter().any(|keyword| keyword.starts_with(trimmed)) {
            return keyword_items(is_scene, is_play, is_theatre);
        }
        Vec::new()
    }

    fn scene_items(&self) -> Vec<CompletionItem> {
        self.index
            .scenes
            .iter()
            .map(|(name, info)| CompletionItem {
                label: name.clone(),
                kind: "file".to_string(),
                detail: Some(info.platforms_summary()),
                insert_text: None,
                sort_text: None,
                filter_text: None,
                documentation: Some(format!(
                    "{} pieces; applies to {}",
                    info.pieces.len(),
                    info.platforms_summary()
                )),
                replace: None,
            })
            .collect()
    }

    fn stage_items(&self) -> Vec<CompletionItem> {
        self.index
            .stages
            .iter()
            .map(|(name, info)| CompletionItem {
                label: name.clone(),
                kind: "function".to_string(),
                detail: Some(format!("{} ({})", info.signature, info.theatre)),
                insert_text: None,
                sort_text: None,
                filter_text: None,
                documentation: info.description.clone(),
                replace: None,
            })
            .collect()
    }

    fn stage_param_items(&self, path: &Path, stage_name: &str) -> Vec<CompletionItem> {
        let Some(info) = self.index.stage(stage_name) else {
            return Vec::new();
        };
        let _ = path;
        info.params
            .iter()
            .map(|param| CompletionItem {
                label: format!("{} = ", param.name),
                kind: "property".to_string(),
                detail: Some(match &param.default {
                    Some(default) => format!("default: '{default}'"),
                    None => "required".to_string(),
                }),
                insert_text: Some(format!("{} = ", param.name)),
                sort_text: None,
                filter_text: None,
                documentation: None,
                replace: None,
            })
            .collect()
    }

    fn piece_items(&self, scene: &str) -> Vec<CompletionItem> {
        let Some(info) = self.index.scenes.get(scene) else {
            return Vec::new();
        };
        let mut pieces = info.pieces.iter().collect::<Vec<_>>();
        pieces.sort_by(|left, right| left.name.cmp(&right.name));
        pieces
            .into_iter()
            .map(|piece| CompletionItem {
                label: piece.name.clone(),
                kind: "reference".to_string(),
                detail: Some(piece.selectors_summary()),
                insert_text: None,
                sort_text: None,
                filter_text: None,
                documentation: None,
                replace: None,
            })
            .collect()
    }

    fn key_items(&self) -> Vec<CompletionItem> {
        let known = self.platforms();
        RemoteKey::ALL
            .iter()
            .copied()
            .filter(|key| {
                known
                    .iter()
                    .any(|platform| support::key_support(*platform, *key).is_supported())
            })
            .map(|key| {
                let supported = known
                    .iter()
                    .filter(|platform| support::key_support(**platform, key).is_supported())
                    .map(|platform| platform.as_str())
                    .collect::<Vec<_>>();
                CompletionItem {
                    label: key.as_str().to_string(),
                    kind: "constant".to_string(),
                    detail: Some(format!("keys: {}", supported.join(", "))),
                    insert_text: None,
                    sort_text: None,
                    filter_text: None,
                    documentation: None,
                    replace: None,
                }
            })
            .collect()
    }

    fn tag_items(&self) -> Vec<CompletionItem> {
        self.index
            .tags
            .iter()
            .map(|tag| simple(tag, "value"))
            .collect()
    }

    fn platform_items(&self, is_scene: bool) -> Vec<CompletionItem> {
        let mut items = Platform::ALL
            .iter()
            .map(|platform| {
                let devices = self
                    .devices
                    .iter()
                    .filter(|device| device.platform == *platform)
                    .map(|device| device.name.as_str())
                    .collect::<Vec<_>>();
                let mut item = simple(platform.as_str(), "value");
                if !devices.is_empty() {
                    item.detail = Some(format!("devices: {}", devices.join(", ")));
                }
                item
            })
            .collect::<Vec<_>>();
        if is_scene {
            items.push(simple("cross-platform", "value"));
        }
        items
    }

    fn app_items(&self) -> Vec<CompletionItem> {
        self.apps
            .iter()
            .map(|app| CompletionItem {
                label: app.name.clone(),
                kind: "class".to_string(),
                detail: app.description.clone(),
                insert_text: None,
                sort_text: None,
                filter_text: None,
                documentation: app.description.clone(),
                replace: None,
            })
            .collect()
    }

    fn table_scene_items(
        &self,
        path: &Path,
        doc: &Doc<'_>,
        position: Position,
        prefix: &str,
    ) -> Option<Vec<CompletionItem>> {
        if !prefix.trim_start().starts_with('|') && !inside_table(doc, position.line) {
            return None;
        }
        if !inside_table(doc, position.line) && !prefix.trim_start().starts_with('|') {
            return None;
        }
        let cells = split_cells(prefix);
        let cell_index = cells.len().saturating_sub(1);
        let current_is_header = is_separator_line(doc.line(position.line + 1));
        let header = table_header(doc, position.line);
        if header.is_none() {
            if current_is_header {
                // While typing the header row, the first cell names the
                // table and every following cell is a platform column.
                if cell_index == 0 {
                    return Some(
                        std::iter::once(simple("Piece", "property"))
                            .chain(
                                Platform::ALL
                                    .iter()
                                    .map(|platform| simple(platform.as_str(), "value")),
                            )
                            .collect(),
                    );
                }
                return Some(
                    Platform::ALL
                        .iter()
                        .map(|platform| simple(platform.as_str(), "value"))
                        .collect(),
                );
            }
            return Some(vec![simple("Piece", "property"), simple("|", "operator")]);
        }
        let header_cells = header.unwrap_or_default();
        if cell_index == 0 {
            let scene = self.scene(path);
            return Some(
                scene
                    .map(|scene| {
                        scene
                            .pieces
                            .iter()
                            .map(|piece| simple(piece.name.as_str(), "reference"))
                            .collect()
                    })
                    .unwrap_or_default(),
            );
        }
        let platform = header_cells.get(cell_index).and_then(|cell| {
            Platform::ALL
                .iter()
                .copied()
                .find(|p| p.as_str() == cell.trim())
        });
        if let Some(platform) = platform {
            let inside = last_quote_is_open(prefix);
            if inside {
                return Some(selector_prefix_items(platform));
            }
            return Some(vec![simple("\"", "operator")]);
        }
        Some(Vec::new())
    }

    fn examples_items(
        &self,
        path: &Path,
        doc: &Doc<'_>,
        position: Position,
        prefix: &str,
    ) -> Option<Vec<CompletionItem>> {
        if !prefix.trim_start().starts_with('|') {
            return None;
        }
        let act_name = enclosing_act(doc, position.line)?;
        let play = self.play(path)?;
        let act = play
            .acts
            .iter()
            .find(|candidate| candidate.name == act_name)?;
        let current = doc.line(position.line);
        let next = doc.line(position.line + 1);
        if !current.trim_start().starts_with('|') || !is_separator_line(next) {
            return Some(Vec::new());
        }
        let mut headers: Vec<(String, Option<String>)> = act
            .params
            .iter()
            .map(|param| {
                (
                    param.name.clone(),
                    param.default.as_ref().map(|d| d.raw.clone()),
                )
            })
            .collect();
        for perform in &act.performs {
            let Some(stage) = self.index.stage(&perform.stage_name) else {
                continue;
            };
            for param in &stage.params {
                if !headers.iter().any(|(name, _)| name == &param.name) {
                    headers.push((param.name.clone(), param.default.clone()));
                }
            }
        }
        Some(
            headers
                .into_iter()
                .map(|(name, default)| {
                    let mut item = simple(name, "variable");
                    item.detail = default;
                    item
                })
                .collect(),
        )
    }

    /// The nearest `Scene:` line above the cursor with a lower indentation.
    #[must_use]
    pub fn enclosing_scene(&self, path: &Path, line: u32) -> Option<String> {
        let text = self.source(path)?;
        enclosing_scene(&Doc::new(text), line, u32::MAX)
    }
}

fn interpolation_items(
    language: &Language,
    path: &Path,
    position: Position,
    prefix: &str,
) -> Option<Vec<CompletionItem>> {
    let start = prefix.rfind("${")?;
    let tail = prefix.get(start + 2..)?;
    if tail.contains('}') {
        return None;
    }
    let (namespace, partial) = match tail.split_once('.') {
        Some((namespace, partial)) => (namespace, partial),
        None => (tail, ""),
    };
    if tail.is_empty() || (partial.is_empty() && !tail.contains('.')) {
        let mut items = vec![
            simple("var.", "namespace"),
            simple("param.", "namespace"),
            simple("env.", "namespace"),
        ];
        if language.play(path).is_some() {
            items.push(simple("example.", "namespace"));
        }
        return Some(items);
    }
    if partial.contains(char::is_whitespace) {
        return Some(Vec::new());
    }
    let names: Vec<String> = match namespace {
        "var" => {
            if let Some(play) = language.play(path) {
                play.var_decls.iter().map(|var| var.name.clone()).collect()
            } else {
                language
                    .workspace
                    .plays
                    .iter()
                    .flat_map(|play| play.var_decls.iter().map(|var| var.name.clone()))
                    .collect()
            }
        }
        "param" => nearest_params(language, path, prefix),
        "env" => language.index.env_names.iter().cloned().collect(),
        "example" => {
            let current = enclosing_act(&Doc::new(language.source(path)?), position.line);
            let mut names = Vec::new();
            if let Some(play) = language.play(path) {
                for act in &play.acts {
                    if current
                        .as_deref()
                        .is_some_and(|current| !current.is_empty() && current != act.name)
                    {
                        continue;
                    }
                    if let Some(table) = &act.examples {
                        names.extend(table.columns.iter().map(|column| column.name.clone()));
                    }
                }
            }
            names
        }
        _ => return Some(Vec::new()),
    };
    Some(
        names
            .into_iter()
            .filter(|name| {
                partial.is_empty()
                    || name
                        .to_ascii_lowercase()
                        .starts_with(&partial.to_ascii_lowercase())
            })
            .map(|name| simple(name, "variable"))
            .collect(),
    )
}

fn nearest_params(language: &Language, path: &Path, prefix: &str) -> Vec<String> {
    let Some(text) = language.source(path) else {
        return Vec::new();
    };
    let doc = Doc::new(text);
    let line = prefix.lines().count() as u32;
    for index in (1..=line.min(doc.end_line())).rev() {
        let content = doc.line(index).trim_start();
        if let Some(rest) = content
            .strip_prefix("Stage:")
            .or_else(|| content.strip_prefix("Act:"))
        {
            if let Some(open) = rest.find('(') {
                if let Some(close) = rest.rfind(')') {
                    return rest
                        .get(open + 1..close)
                        .unwrap_or("")
                        .split(',')
                        .filter_map(|param| param.split('=').next().map(str::trim))
                        .filter(|name| !name.is_empty())
                        .map(ToOwned::to_owned)
                        .collect();
                }
            }
        }
    }
    Vec::new()
}

fn simple(label: impl Into<String>, kind: &str) -> CompletionItem {
    CompletionItem {
        label: label.into(),
        kind: kind.to_string(),
        detail: None,
        insert_text: None,
        sort_text: None,
        filter_text: None,
        documentation: None,
        replace: None,
    }
}

fn keyword_items(is_scene: bool, is_play: bool, is_theatre: bool) -> Vec<CompletionItem> {
    let mut available = vec!["Description:", "Tags:"];
    if is_scene {
        available.extend(["Scene:", "Platforms:"]);
    }
    if is_play {
        available.extend(["Play:", "App:", "Var:", "Act:"]);
    }
    if is_theatre {
        available.extend(["Theatre:", "Stage:"]);
    }
    available
        .into_iter()
        .map(|keyword| simple(keyword, "keyword"))
        .collect()
}

fn enclosing_scene(doc: &Doc<'_>, line: u32, _col: u32) -> Option<String> {
    let indent = doc.indent(line);
    for index in (1..line).rev() {
        let content = doc.line(index).trim();
        if content.is_empty() {
            continue;
        }
        if doc.indent(index) < indent {
            if let Some(name) = content.strip_prefix("Scene:") {
                return Some(name.trim().to_string());
            }
        }
    }
    None
}

fn enclosing_act(doc: &Doc<'_>, line: u32) -> Option<String> {
    let indent = doc.indent(line);
    for index in (1..line).rev() {
        let content = doc.line(index).trim();
        if content.is_empty() {
            continue;
        }
        if doc.indent(index) <= indent {
            if let Some(name) = content.strip_prefix("Act:") {
                return Some(name.split('(').next().unwrap_or("").trim().to_string());
            }
        }
    }
    None
}

fn inside_table(doc: &Doc<'_>, line: u32) -> bool {
    let mut index = line;
    while index > 1 {
        index -= 1;
        let content = doc.line(index).trim();
        if content.is_empty() {
            continue;
        }
        if content.starts_with('|') {
            return true;
        }
        if !content.starts_with('|') && !content.starts_with('-') {
            return false;
        }
    }
    false
}

fn table_header(doc: &Doc<'_>, line: u32) -> Option<Vec<String>> {
    // The line above a separator is the header itself; callers use `None`
    // there to suggest the first-column/header keywords.
    if is_separator_line(doc.line(line + 1)) {
        return None;
    }
    let mut index = line;
    while index > 1 {
        index -= 1;
        let content = doc.line(index).trim();
        if !content.starts_with('|') {
            continue;
        }
        if is_separator_line(content) {
            continue;
        }
        return Some(split_cells(content).into_iter().collect());
    }
    None
}

fn is_separator_line(content: &str) -> bool {
    let trimmed = content.trim();
    trimmed.starts_with('|')
        && trimmed
            .chars()
            .all(|ch| ch == '|' || ch == '-' || ch == ':' || ch == ' ')
}

fn split_cells(prefix: &str) -> Vec<String> {
    prefix
        .trim()
        .trim_start_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

fn last_quote_is_open(prefix: &str) -> bool {
    let mut open = false;
    for ch in prefix.chars() {
        if ch == '"' {
            open = !open;
        }
    }
    open
}

fn condition_items() -> Vec<CompletionItem> {
    let mut items = [
        CheckVerb::Visible,
        CheckVerb::Hidden,
        CheckVerb::Enabled,
        CheckVerb::Disabled,
        CheckVerb::Focused,
    ]
    .into_iter()
    .map(|verb| simple(format!("is {}", verb.as_str()), "keyword"))
    .collect::<Vec<_>>();
    for verb in [
        CheckVerb::TextEquals,
        CheckVerb::TextContains,
        CheckVerb::Count,
    ] {
        items.push(simple(verb.as_str(), "keyword"));
    }
    items.push(simple("timeout", "keyword"));
    items
}

fn verb_items() -> Vec<CompletionItem> {
    let mut items = VERBS
        .iter()
        .map(|verb| simple(verb.as_str(), "keyword"))
        .collect::<Vec<_>>();
    let mut starter = simple("'", "snippet");
    starter.insert_text = Some("''".to_string());
    items.push(starter);
    items
}

fn duration_items() -> Vec<CompletionItem> {
    DURATIONS
        .iter()
        .map(|duration| simple(*duration, "value"))
        .collect()
}

fn direction_items() -> Vec<CompletionItem> {
    ["up", "down", "left", "right"]
        .into_iter()
        .map(|direction| simple(direction, "enumMember"))
        .collect()
}

fn app_op_items() -> Vec<CompletionItem> {
    ["launch", "terminate", "restart", "background"]
        .into_iter()
        .map(|operation| simple(operation, "enumMember"))
        .collect()
}

fn selector_prefix_items(key: Platform) -> Vec<CompletionItem> {
    let prefixes: &[&str] = match key {
        Platform::Android | Platform::AndroidTv => {
            &["id:", "text:", "textContains:", "desc:", "class:"]
        }
        Platform::Ios | Platform::Tvos => &[
            "accessibilityId:",
            "label:",
            "labelContains:",
            "predicate:",
            "type:",
        ],
        Platform::Web | Platform::WebOs | Platform::Tizen => {
            &["css:", "xpath:", "text:", "textContains:"]
        }
    };
    prefixes
        .iter()
        .map(|prefix| simple(*prefix, "enumMember"))
        .collect()
}

fn extension(path: &Path) -> &str {
    path.extension()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or_default()
}
