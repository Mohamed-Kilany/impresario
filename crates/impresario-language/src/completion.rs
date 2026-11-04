//! Context-sensitive completion for the current DSL (TASK-2 section 9.1).
//!
//! Completion is computed from the line prefix, the nearest enclosing `Scene:`
//! or `Act:` line, and the workspace index. It deliberately does not require
//! the rest of the file to parse, so it keeps working while the user types.

use std::collections::BTreeSet;
use std::path::Path;

use impresario_core::keys::RemoteKey;
use impresario_core::platforms::Platform;
use impresario_core::probe::{self, DrmProvider, PlayerStatus, ProbeProperty};
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
    "Actors:",
    "Audience:",
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
            return self.stage_items_for(path, &doc, position, None);
        }

        if let Some(rest) = trimmed.strip_prefix("Perform For [") {
            return self.stage_items_after_filter(path, &doc, position, rest, StageFilterKind::For);
        }
        if let Some(rest) = trimmed.strip_prefix("Perform Except [") {
            return self.stage_items_after_filter(
                path,
                &doc,
                position,
                rest,
                StageFilterKind::Except,
            );
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
            return self.check_items(path, &doc, position, rest);
        }

        if trimmed.starts_with("Tags:") {
            return self.tag_items();
        }
        if trimmed.starts_with("Actors:") && is_play {
            return self.actor_items();
        }
        if trimmed.starts_with("Audience:") && is_theatre {
            return self.platform_items(true);
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

    fn stage_items_after_filter(
        &self,
        path: &Path,
        doc: &Doc<'_>,
        position: Position,
        rest: &str,
        kind: StageFilterKind,
    ) -> Vec<CompletionItem> {
        let Some((actors, after)) = rest.split_once(']') else {
            return self.actor_items();
        };
        let after = after.trim_start();
        let Some(after) = after.strip_prefix(':') else {
            return self.actor_items();
        };
        let after = after.trim_start();
        if let Some((stage_name, arguments)) = after.split_once('(') {
            if !arguments.contains(')') {
                return self.stage_param_items(path, stage_name.trim());
            }
        }
        self.stage_items_for(path, doc, position, Some((kind, actors)))
    }

    /// Stage completion narrowed by the act's effective actors and the optional
    /// `Perform For/Except` filter. Stages without any effective implementation
    /// are omitted; stages missing at least one actor are retained and marked.
    fn stage_items_for(
        &self,
        path: &Path,
        doc: &Doc<'_>,
        position: Position,
        filter: Option<(StageFilterKind, &str)>,
    ) -> Vec<CompletionItem> {
        let Some(devices) = self.effective_actor_devices(path, doc, position, filter) else {
            return self.stage_items();
        };
        if devices.is_empty() {
            return self.stage_items();
        }
        let total = devices.len();
        let mut items = self
            .index
            .stages
            .iter()
            .filter_map(|(name, info)| {
                let missing = devices
                    .iter()
                    .filter(|device| self.workspace.stage_for(name, device.platform).is_none())
                    .collect::<Vec<_>>();
                let covered = total.saturating_sub(missing.len());
                if covered == 0 {
                    return None;
                }
                let coverage = if missing.is_empty() {
                    format!("all {total} actor(s)")
                } else {
                    format!("partial: {covered}/{total} actors")
                };
                let mut documentation = info.description.clone().unwrap_or_default();
                if !missing.is_empty() {
                    if !documentation.is_empty() {
                        documentation.push_str("\n\n");
                    }
                    documentation.push_str("Missing stage implementations for: ");
                    documentation.push_str(
                        &missing
                            .iter()
                            .map(|device| format!("{} ({})", device.name, device.platform.as_str()))
                            .collect::<Vec<_>>()
                            .join(", "),
                    );
                }
                Some(CompletionItem {
                    label: name.clone(),
                    kind: "function".to_string(),
                    detail: Some(format!(
                        "{} ({}) · {coverage}",
                        info.signature, info.theatre
                    )),
                    insert_text: None,
                    sort_text: Some(if missing.is_empty() { "0" } else { "1" }.to_string()),
                    filter_text: None,
                    documentation: if documentation.is_empty() {
                        None
                    } else {
                        Some(documentation)
                    },
                    replace: None,
                })
            })
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            left.sort_text
                .cmp(&right.sort_text)
                .then_with(|| left.label.cmp(&right.label))
        });
        items
    }

    fn effective_actor_devices(
        &self,
        path: &Path,
        doc: &Doc<'_>,
        position: Position,
        filter: Option<(StageFilterKind, &str)>,
    ) -> Option<Vec<&crate::model::DeviceInfo>> {
        let act_name = enclosing_act(doc, position.line)?;
        let play = self.play(path)?;
        let act = play
            .acts
            .iter()
            .find(|candidate| candidate.name == act_name)?;
        let references = play.effective_actors(act);
        let mut effective = BTreeSet::new();
        if references.is_empty() {
            effective.extend(self.devices.iter().map(|device| device.name.clone()));
        } else {
            for named in references {
                self.expand_actor_name_into(&named.name, &mut effective, &mut Vec::new());
            }
        }
        if effective.is_empty() {
            return None;
        }
        if let Some((kind, actors)) = filter {
            let mut selected = BTreeSet::new();
            for reference in actors.split(',') {
                let reference = reference.trim();
                if !reference.is_empty() {
                    self.expand_actor_name_into(reference, &mut selected, &mut Vec::new());
                }
            }
            if selected.is_empty() {
                return None;
            }
            match kind {
                StageFilterKind::For => effective.retain(|actor| selected.contains(actor)),
                StageFilterKind::Except => effective.retain(|actor| !selected.contains(actor)),
            }
        }
        let act_platforms = act
            .platforms
            .iter()
            .filter_map(|named| named.name.parse::<Platform>().ok())
            .collect::<BTreeSet<_>>();
        let devices = self
            .devices
            .iter()
            .filter(|device| {
                effective.contains(&device.name)
                    && (act_platforms.is_empty() || act_platforms.contains(&device.platform))
            })
            .collect::<Vec<_>>();
        if devices.is_empty() {
            None
        } else {
            Some(devices)
        }
    }

    fn expand_actor_name_into(
        &self,
        reference: &str,
        out: &mut BTreeSet<String>,
        stack: &mut Vec<String>,
    ) {
        if let Some(group) = reference.strip_prefix('@') {
            if stack.iter().any(|item| item == group) {
                return;
            }
            if let Some(members) = self.groups.get(group) {
                stack.push(group.to_string());
                for member in members {
                    self.expand_actor_name_into(member, out, stack);
                }
                stack.pop();
            }
        } else if self.devices.iter().any(|device| device.name == reference) {
            out.insert(reference.to_string());
        }
    }

    fn check_items(
        &self,
        path: &Path,
        doc: &Doc<'_>,
        position: Position,
        after: &str,
    ) -> Vec<CompletionItem> {
        let platforms = self.effective_platforms(path, doc, position);
        let pieces = enclosing_scene(doc, position.line, position.col)
            .map_or_else(Vec::new, |scene| self.piece_items(&scene));
        let trimmed = after.trim_start();
        if let Some(rest) = after_keyword(trimmed, "not") {
            let rest = rest.trim_start();
            if let Some(showing) = after_keyword(rest, "Showing") {
                return self.showing_items(showing, pieces, &platforms);
            }
            return vec![simple("Showing", "keyword")];
        }
        if let Some(rest) = after_keyword(trimmed, "Showing") {
            return self.showing_items(rest, pieces, &platforms);
        }
        if let Some(rest) = after_keyword(trimmed, "Player") {
            return self.player_items(rest, pieces, &platforms);
        }
        if trimmed.is_empty() || !trimmed.contains(char::is_whitespace) {
            let mut items = pieces;
            items.push(simple("Showing", "keyword"));
            items.push(simple("Player", "keyword"));
            items.push(simple("not", "keyword"));
            return items;
        }
        condition_after_piece(trimmed)
    }

    fn effective_platforms(&self, path: &Path, doc: &Doc<'_>, position: Position) -> Vec<Platform> {
        let mut platforms = BTreeSet::new();
        if let Some(devices) = self.effective_actor_devices(path, doc, position, None) {
            platforms.extend(devices.iter().map(|device| device.platform));
        }
        if platforms.is_empty() {
            if let Some(theatre) = self.theatre(path) {
                let line = position.line as usize;
                if let Some(stage) = theatre
                    .stages
                    .iter()
                    .rev()
                    .find(|stage| stage.name_span.line <= line)
                {
                    platforms.extend(theatre.stage_audience(stage));
                } else {
                    platforms.extend(
                        theatre
                            .audience
                            .iter()
                            .filter_map(|named| named.name.parse::<Platform>().ok()),
                    );
                }
            }
        }
        if platforms.is_empty() {
            platforms.extend(self.platforms());
        }
        platforms.into_iter().collect()
    }

    fn showing_items(
        &self,
        rest: &str,
        pieces: Vec<CompletionItem>,
        platforms: &[Platform],
    ) -> Vec<CompletionItem> {
        let rest = rest.trim_start();
        let Some(after) = rest.strip_prefix("=>") else {
            return vec![simple("=>", "operator")];
        };
        let after = after.trim_start();
        if after.is_empty() || !after.contains(char::is_whitespace) {
            return pieces;
        }
        let (_, tail) = split_first_token(after);
        let tail = tail.trim_start();
        if tail.is_empty() {
            return vec![simple("with", "keyword"), simple("timeout", "keyword")];
        }
        if let Some(content) = after_keyword(tail, "with") {
            return with_items(content, platforms, true);
        }
        if let Some(content) = after_keyword(tail, "timeout") {
            return timeout_items(content);
        }
        if "with".starts_with(tail) || "timeout".starts_with(tail) {
            return vec![simple("with", "keyword"), simple("timeout", "keyword")];
        }
        let mut items = vec![simple(",", "operator"), simple("timeout", "keyword")];
        items.extend(showing_option_items(platforms));
        items
    }

    fn player_items(
        &self,
        rest: &str,
        pieces: Vec<CompletionItem>,
        platforms: &[Platform],
    ) -> Vec<CompletionItem> {
        let rest = rest.trim_start();
        let Some(after) = rest.strip_prefix("=>") else {
            return vec![simple("=>", "operator")];
        };
        let after = after.trim_start();
        if after.is_empty() || !after.contains(char::is_whitespace) {
            return pieces;
        }
        let (_, tail) = split_first_token(after);
        let tail = tail.trim_start();
        if tail.is_empty() {
            return player_tail_items(true, true);
        }
        if let Some(index) = find_keyword(tail, "with") {
            return with_items(&tail[index + "with".len()..], platforms, false);
        }
        if let Some(index) = find_keyword(tail, "timeout") {
            return timeout_items(&tail[index + "timeout".len()..]);
        }
        let trailing = tail.chars().last().is_some_and(char::is_whitespace);
        let drm = ["DRM-enabled", "DRM-disabled"]
            .into_iter()
            .filter_map(|keyword| find_keyword(tail, keyword).map(|index| (index, keyword)))
            .max_by_key(|(index, _)| *index);
        if let Some((index, keyword)) = drm {
            let after_drm = tail[index + keyword.len()..].trim_start();
            let provider_context = match after_drm.split_whitespace().collect::<Vec<_>>().as_slice()
            {
                [] | ["not"] => true,
                [provider] | ["not", provider] => {
                    !trailing && DrmProvider::parse(provider).is_none()
                }
                _ => false,
            };
            if provider_context {
                let mut items = provider_items(platforms);
                items.extend(player_tail_items(!player_has_status(tail), false));
                return items;
            }
            return player_tail_items(!player_has_status(tail), false);
        }
        let tokens = tail.split_whitespace().collect::<Vec<_>>();
        if tokens.last() == Some(&"not") {
            return status_items();
        }
        if tokens
            .iter()
            .any(|token| PlayerStatus::parse(token).is_some())
        {
            return player_tail_items(false, true);
        }
        player_tail_items(!player_has_status(tail), !player_has_drm(tail))
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

    fn actor_items(&self) -> Vec<CompletionItem> {
        let mut items: Vec<CompletionItem> = self
            .devices
            .iter()
            .map(|device| CompletionItem {
                label: device.name.clone(),
                kind: "actor".to_string(),
                detail: Some(device.platform.as_str().to_string()),
                insert_text: None,
                sort_text: None,
                filter_text: None,
                documentation: None,
                replace: None,
            })
            .collect();
        items.extend(self.groups.iter().map(|(name, members)| CompletionItem {
            label: format!("@{name}"),
            kind: "actor".to_string(),
            detail: Some(format!("group: {}", members.join(", "))),
            insert_text: None,
            sort_text: None,
            filter_text: None,
            documentation: None,
            replace: None,
        }));
        items.sort_by(|left, right| left.label.cmp(&right.label));
        items
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
            return Some(selector_prefix_items(platform));
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
        available.extend(["Play:", "App:", "Actors:", "Var:", "Act:"]);
    }
    if is_theatre {
        available.extend(["Theatre:", "Audience:", "Stage:"]);
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

fn after_keyword<'a>(value: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = value.strip_prefix(keyword)?;
    if rest.is_empty() || rest.chars().next().is_some_and(char::is_whitespace) {
        Some(rest)
    } else {
        None
    }
}

fn find_keyword(value: &str, keyword: &str) -> Option<usize> {
    value.match_indices(keyword).find_map(|(index, _)| {
        let before = value[..index].chars().next_back();
        let after = value[index + keyword.len()..].chars().next();
        (before.is_none_or(char::is_whitespace) && after.is_none_or(char::is_whitespace))
            .then_some(index)
    })
}

fn split_first_token(value: &str) -> (&str, &str) {
    match value.find(char::is_whitespace) {
        Some(index) => (&value[..index], value[index..].trim_start()),
        None => (value, ""),
    }
}

fn condition_after_piece(trimmed: &str) -> Vec<CompletionItem> {
    let (_, tail) = split_first_token(trimmed);
    let tail = tail.trim_start();
    if let Some(rest) = after_keyword(tail, "is") {
        let rest = rest.trim_start();
        if rest.is_empty() || !rest.contains(char::is_whitespace) {
            return ["visible", "hidden", "enabled", "disabled", "focused"]
                .into_iter()
                .map(|condition| simple(condition, "enumMember"))
                .collect();
        }
    }
    if let Some(rest) = after_keyword(tail, "text") {
        if rest.chars().next().is_some_and(char::is_whitespace) && rest.trim().is_empty() {
            let mut item = simple("\"\"", "snippet");
            item.insert_text = Some("\"\"".to_string());
            return vec![item];
        }
    }
    if let Some(rest) = after_keyword(tail, "textContains") {
        if rest.chars().next().is_some_and(char::is_whitespace) && rest.trim().is_empty() {
            let mut item = simple("\"\"", "snippet");
            item.insert_text = Some("\"\"".to_string());
            return vec![item];
        }
    }
    if let Some(rest) = after_keyword(tail, "count") {
        if rest.chars().next().is_some_and(char::is_whitespace) && rest.trim().is_empty() {
            return ["1", "2", "3"]
                .into_iter()
                .map(|count| simple(count, "value"))
                .collect();
        }
    }
    condition_items()
}

fn status_items() -> Vec<CompletionItem> {
    PlayerStatus::ALL
        .iter()
        .map(|status| {
            let mut item = simple(status.as_str(), "enumMember");
            item.detail = Some("player status".to_string());
            item
        })
        .collect()
}

fn player_has_status(tail: &str) -> bool {
    tail.split_whitespace()
        .any(|word| PlayerStatus::parse(word).is_some())
}

fn player_has_drm(tail: &str) -> bool {
    tail.split_whitespace()
        .any(|word| word == "DRM-enabled" || word == "DRM-disabled")
}

fn player_tail_items(status: bool, drm: bool) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    if status {
        items.extend(status_items());
        items.push(simple("not", "keyword"));
    }
    if drm {
        items.push(simple("DRM-enabled", "keyword"));
        items.push(simple("DRM-disabled", "keyword"));
    }
    items.push(simple("with", "keyword"));
    items.push(simple("timeout", "keyword"));
    items
}

fn supported_providers(platforms: &[Platform]) -> Vec<DrmProvider> {
    let Some((first, rest)) = platforms.split_first() else {
        return DrmProvider::ALL.to_vec();
    };
    let mut providers = probe::support(*first).providers;
    for platform in rest {
        let support = probe::support(*platform);
        providers.retain(|provider| support.supports_provider(*provider));
    }
    providers
}

fn provider_items(platforms: &[Platform]) -> Vec<CompletionItem> {
    supported_providers(platforms)
        .into_iter()
        .map(|provider| {
            let mut item = simple(provider.as_str(), "enumMember");
            item.detail = Some("DRM provider".to_string());
            item
        })
        .collect()
}

fn supported_properties(platforms: &[Platform]) -> Vec<ProbeProperty> {
    let Some((first, rest)) = platforms.split_first() else {
        return ProbeProperty::ALL.to_vec();
    };
    let mut properties = probe::support(*first).properties;
    for platform in rest {
        let support = probe::support(*platform);
        properties.retain(|property| support.supports_property(*property));
    }
    properties
}

fn property_items(platforms: &[Platform]) -> Vec<CompletionItem> {
    supported_properties(platforms)
        .into_iter()
        .map(|property| {
            let mut item = simple(property.as_str(), "property");
            item.detail = Some("player property".to_string());
            item.documentation = Some(property_value_help(property).to_string());
            item
        })
        .collect()
}

fn property_items_after_not(platforms: &[Platform]) -> Vec<CompletionItem> {
    let mut items = vec![simple("not", "keyword")];
    items.extend(property_items(platforms));
    items
}

fn property_value_help(property: ProbeProperty) -> &'static str {
    match property {
        ProbeProperty::Position | ProbeProperty::Duration | ProbeProperty::Buffered => {
            "duration value, e.g. `30s` or `1m`"
        }
        ProbeProperty::Rate => "playback rate, e.g. `1.0` or `0.5`",
        ProbeProperty::Volume => "volume from 0.0 to 1.0",
        ProbeProperty::Muted
        | ProbeProperty::Live
        | ProbeProperty::Looping
        | ProbeProperty::Fullscreen
        | ProbeProperty::Secure => "boolean value: `true` or `false`",
        ProbeProperty::Width | ProbeProperty::Height => "pixel value, e.g. `1920`",
        ProbeProperty::Resolution => "resolution, e.g. `1080p` or `2160p`",
        ProbeProperty::Bitrate => "bitrate, e.g. `5000kbps`",
        ProbeProperty::Framerate => "frames per second, e.g. `60`",
        ProbeProperty::DroppedFrames => "dropped-frame count, e.g. `0` or `50`",
        ProbeProperty::VideoCodec => "video codec, e.g. `h264` or `h265`",
        ProbeProperty::AudioCodec => "audio codec, e.g. `aac` or `opus`",
        ProbeProperty::Decoder => "decoder kind: `hardware` or `software`",
        ProbeProperty::AudioTrack | ProbeProperty::SubtitleTrack => "track language tag",
        ProbeProperty::Error => "error text or `none`",
        ProbeProperty::License => "license state, e.g. `active`",
        ProbeProperty::KeyStatus => "key status, e.g. `usable`",
        ProbeProperty::SecurityLevel => "security level, e.g. `L1`",
        ProbeProperty::KeySystem => "EME key system id",
        ProbeProperty::Hdcp => "HDCP version, e.g. `HDCP-2.2`",
    }
}

fn operator_items(property: ProbeProperty) -> Vec<CompletionItem> {
    let operators: &[&str] = match property {
        ProbeProperty::Muted
        | ProbeProperty::Live
        | ProbeProperty::Looping
        | ProbeProperty::Fullscreen
        | ProbeProperty::Secure => &["=", "!="],
        ProbeProperty::VideoCodec
        | ProbeProperty::AudioCodec
        | ProbeProperty::Decoder
        | ProbeProperty::AudioTrack
        | ProbeProperty::SubtitleTrack
        | ProbeProperty::Error
        | ProbeProperty::License
        | ProbeProperty::KeyStatus
        | ProbeProperty::SecurityLevel
        | ProbeProperty::KeySystem
        | ProbeProperty::Hdcp => &["=", "!=", "~"],
        _ => &[">", ">=", "<", "<=", "=", "!="],
    };
    let mut items = operators
        .iter()
        .map(|operator| simple(*operator, "operator"))
        .collect::<Vec<_>>();
    items.push(simple(",", "operator"));
    items.push(simple("timeout", "keyword"));
    items
}

fn property_value_items(property: ProbeProperty) -> Vec<CompletionItem> {
    let values: &[&str] = match property {
        ProbeProperty::Position | ProbeProperty::Duration | ProbeProperty::Buffered => {
            &["30s", "10s", "0s"]
        }
        ProbeProperty::Rate => &["1.0", "0.5", "2.0"],
        ProbeProperty::Volume => &["1.0", "0.5", "0.0"],
        ProbeProperty::Muted
        | ProbeProperty::Live
        | ProbeProperty::Looping
        | ProbeProperty::Fullscreen
        | ProbeProperty::Secure => &["true", "false"],
        ProbeProperty::Width | ProbeProperty::Height => &["1920", "1080"],
        ProbeProperty::Resolution => &["1080p", "720p", "2160p"],
        ProbeProperty::Bitrate => &["5000kbps", "300kbps"],
        ProbeProperty::Framerate => &["60", "30"],
        ProbeProperty::DroppedFrames => &["0", "50"],
        ProbeProperty::VideoCodec => &["h264", "h265", "av1", "vp9"],
        ProbeProperty::AudioCodec => &["aac", "opus", "ac3", "eac3"],
        ProbeProperty::Decoder => &["hardware", "software"],
        ProbeProperty::AudioTrack | ProbeProperty::SubtitleTrack => &["en", "de"],
        ProbeProperty::Error => &["none"],
        ProbeProperty::License => &["active", "expired", "none"],
        ProbeProperty::KeyStatus => &["usable", "expired", "output-restricted"],
        ProbeProperty::SecurityLevel => &["L1", "L2", "L3"],
        ProbeProperty::KeySystem => &[
            "com.widevine.alpha",
            "com.microsoft.playready",
            "com.apple.fps",
        ],
        ProbeProperty::Hdcp => &["HDCP-1.4", "HDCP-2.2", "none"],
    };
    values
        .iter()
        .map(|value| {
            let mut item = simple(*value, "value");
            item.detail = Some(format!("{} value", property.as_str()));
            item
        })
        .collect()
}

fn leading_operator(value: &str) -> Option<(&str, &str)> {
    for operator in [">=", "<=", "!=", "=", ">", "<", "~"] {
        if let Some(rest) = value.strip_prefix(operator) {
            return Some((operator, rest));
        }
    }
    None
}

fn with_items(after_with: &str, platforms: &[Platform], showing: bool) -> Vec<CompletionItem> {
    let content = after_with.trim_start();
    let segment = match content.rfind(',') {
        Some(index) => content.get(index + 1..).unwrap_or("").trim_start(),
        None => content,
    };
    if showing {
        showing_with_items(segment, platforms)
    } else {
        player_with_items(segment, platforms)
    }
}

fn with_next_items(platforms: &[Platform], showing: bool) -> Vec<CompletionItem> {
    let mut items = vec![simple(",", "operator"), simple("timeout", "keyword")];
    if showing {
        items.extend(showing_option_items(platforms));
    } else {
        items.extend(property_items_after_not(platforms));
    }
    items
}

fn showing_with_items(segment: &str, platforms: &[Platform]) -> Vec<CompletionItem> {
    let trailing = segment.chars().last().is_some_and(char::is_whitespace);
    let body = segment.trim_end();
    if body.is_empty() {
        return showing_option_items(platforms);
    }
    if let Some(value) = body.strip_prefix("coverage>=") {
        let value = value.trim_start();
        if !value.contains('%') {
            return ["50%", "80%", "100%"]
                .into_iter()
                .map(|percent| simple(percent, "value"))
                .collect();
        }
        return with_next_items(platforms, true);
    }
    if "coverage".starts_with(body) {
        let mut item = simple("coverage>=50%", "snippet");
        item.insert_text = Some("coverage>=50%".to_string());
        return vec![item];
    }
    if let Some(value) = body.strip_prefix("source~") {
        let value = value.trim_start();
        if !value.starts_with('"') || (!value.ends_with('"') && value.len() > 1) {
            let mut item = simple("\"\"", "snippet");
            item.insert_text = Some("\"\"".to_string());
            return vec![item];
        }
        return with_next_items(platforms, true);
    }
    if "source".starts_with(body) {
        let mut item = simple("source~\"\"", "snippet");
        item.insert_text = Some("source~\"\"".to_string());
        return vec![item];
    }
    if body == "decoded" && trailing {
        return with_next_items(platforms, true);
    }
    if "decoded".starts_with(body) {
        return vec![simple("decoded", "keyword")];
    }
    with_next_items(platforms, true)
}

fn player_with_items(segment: &str, platforms: &[Platform]) -> Vec<CompletionItem> {
    let trailing = segment.chars().last().is_some_and(char::is_whitespace);
    let body = segment.trim_end();
    if body.is_empty() || body == "not" {
        return property_items_after_not(platforms);
    }
    let body = if let Some(rest) = after_keyword(body, "not") {
        rest.trim_start()
    } else {
        body
    };
    player_assertion_items(body, platforms, trailing)
}

fn split_property(body: &str) -> Option<(ProbeProperty, &str)> {
    for property in ProbeProperty::ALL {
        let name = property.as_str();
        let Some(rest) = body.strip_prefix(name) else {
            continue;
        };
        if rest.is_empty()
            || rest.starts_with(char::is_whitespace)
            || rest
                .chars()
                .next()
                .is_some_and(|ch| matches!(ch, '>' | '<' | '=' | '!' | '~'))
        {
            return Some((property, rest));
        }
    }
    None
}

fn player_assertion_items(
    body: &str,
    platforms: &[Platform],
    trailing: bool,
) -> Vec<CompletionItem> {
    let Some((property, rest)) = split_property(body) else {
        return property_items(platforms);
    };
    let rest = rest.trim_start();
    if rest.is_empty() {
        if body.contains(char::is_whitespace) || trailing {
            return operator_items(property);
        }
        return property_items(platforms);
    }
    if let Some((_, value)) = leading_operator(rest) {
        if value.trim_start().is_empty() {
            return property_value_items(property);
        }
        if trailing {
            return with_next_items(platforms, false);
        }
        return property_value_items(property);
    }
    property_value_items(property)
}

fn showing_option_items(platforms: &[Platform]) -> Vec<CompletionItem> {
    if !platforms
        .iter()
        .all(|platform| probe::support(*platform).showing)
    {
        return Vec::new();
    }
    let mut items = Vec::new();
    let mut coverage = simple("coverage>=50%", "snippet");
    coverage.insert_text = Some("coverage>=50%".to_string());
    items.push(coverage);
    items.push(simple("decoded", "keyword"));
    if platforms
        .iter()
        .all(|platform| matches!(platform, Platform::Web | Platform::WebOs | Platform::Tizen))
    {
        let mut source = simple("source~\"\"", "snippet");
        source.insert_text = Some("source~\"\"".to_string());
        items.push(source);
    }
    items
}

fn timeout_items(content: &str) -> Vec<CompletionItem> {
    let content = content.trim_start();
    if content.is_empty() || !content.contains(char::is_whitespace) {
        duration_items()
    } else {
        Vec::new()
    }
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

#[derive(Debug, Clone, Copy)]
enum StageFilterKind {
    For,
    Except,
}

fn extension(path: &Path) -> &str {
    path.extension()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or_default()
}
