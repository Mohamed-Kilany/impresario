//! Hover, navigation, refactoring, symbols, folding, semantic tokens,
//! signature help, inlay hints and code lenses.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use impresario_core::ast::{
    ActDecl, ActionVerb, CheckCondition, PerformDecl, PlayFile, SceneBlock, SceneFile, StageDecl,
    StageItem, TheatreFile,
};
use impresario_core::platforms::Platform;
use impresario_core::probe::{self, Confidence};
use impresario_core::scanner;
use impresario_core::span::Span;
use impresario_core::support::{self, Verb};
use serde_json::json;

use crate::model::{Index, SymbolKey};
use crate::service::{
    CodeLens, Command, DeviceConfigSource, DocumentSymbol, FoldingRange, HoverResult, InlayHint,
    Language, PrepareRename, SemanticToken, Signature, SignatureHelp, SignatureParameter, TextEdit,
    WorkspaceEdit,
};
use crate::text::{Doc, Position};
use yaml_edit::Document;

impl Language {
    /// Resolve the symbol under a position.
    #[must_use]
    pub fn symbol_at(&self, path: &Path, position: Position) -> Option<&crate::model::Occurrence> {
        self.index
            .occurrence_at(path, position.line as usize, position.col as usize)
    }

    /// Hover at a position.
    #[must_use]
    pub fn hover(&self, path: &Path, position: Position) -> Option<HoverResult> {
        if let Some(occurrence) = self.symbol_at(path, position) {
            let markdown = match &occurrence.key {
                SymbolKey::Actor(name) => self.actor_markdown(name, Some(&occurrence.span)),
                SymbolKey::Group(name) => self.group_markdown(name, Some(&occurrence.span)),
                _ => self.markdown_for_key(&occurrence.key),
            };
            return Some(HoverResult {
                markdown,
                span: Some(occurrence.span.clone()),
            });
        }
        // The action/check verb itself is not a symbol but has a support
        // verdict for every configured platform.
        if let Some((verb, span)) = self.verb_at(path, position.line) {
            return Some(HoverResult {
                markdown: self.support_markdown(verb),
                span: Some(span),
            });
        }
        None
    }

    fn markdown_for_key(&self, key: &SymbolKey) -> String {
        match key {
            SymbolKey::Scene(name) => {
                let Some(scene) = self.index.scenes.get(name) else {
                    return format!("**{name}**\n\nScene not found.");
                };
                let uses = self.index.references(key, false).len();
                let mut out = format!(
                    "**{}** — scene\n\nPlatforms: `{}`\n\n{} pieces · {} references\n\n",
                    scene.name,
                    match &scene.platforms {
                        impresario_core::platforms::ScenePlatforms::CrossPlatform => {
                            "cross-platform".to_string()
                        }
                        impresario_core::platforms::ScenePlatforms::List(list) => list
                            .iter()
                            .map(|platform| platform.as_str())
                            .collect::<Vec<_>>()
                            .join(", "),
                    },
                    scene.pieces.len(),
                    uses
                );
                if !scene.pieces.is_empty() {
                    out.push_str("| Piece | Selectors |\n|---|---|\n");
                    for piece in &scene.pieces {
                        out.push_str(&format!(
                            "| {} | {} |\n",
                            piece.name,
                            piece.selectors_summary().replace('|', "\\|")
                        ));
                    }
                }
                out
            }
            SymbolKey::Piece { scene, name } => {
                let Some(piece) = self.index.piece(scene, name) else {
                    return format!("**{name}**\n\nPiece not found in `{scene}`.");
                };
                let mut out = format!("**{}**\n\n| Platform | Selector |\n|---|---|\n", piece.name);
                // Show every platform so fallback resolution is visible even
                // when the workspace only configures one device.
                for platform in Platform::ALL {
                    let resolved = resolve_selector(piece, platform);
                    let cell = match resolved {
                        Some((selector, via)) if via == platform => {
                            format!("`{}`", selector.raw.replace('|', "\\|"))
                        }
                        Some((selector, via)) => format!(
                            "`{}` (fallback from {})",
                            selector.raw.replace('|', "\\|"),
                            via.as_str()
                        ),
                        None => "—".to_string(),
                    };
                    out.push_str(&format!("| {} | {} |\n", platform.as_str(), cell));
                }
                out.push_str(&format!(
                    "\nRaw selectors: {}\n\nDeclared in scene `{}`.",
                    piece.selectors_summary(),
                    scene
                ));
                out
            }
            SymbolKey::Stage(name) => {
                let instances = self.workspace.stage_instances(name);
                let Some(stage) = self.index.stage(name) else {
                    return format!("**{name}**\n\nStage not found.");
                };
                let uses = self.index.references(key, false).len();
                let mut out = format!(
                    "**{}** — stage\n\nSignature: `{}`\n\n{} implementation(s):\n\n\
                     | Theatre | Audience | File |\n|---|---|---|\n",
                    stage.name,
                    stage.signature,
                    instances.len()
                );
                for &(theatre, implementation) in &instances {
                    out.push_str(&format!(
                        "| {} | {} | `{}` |\n",
                        theatre.name,
                        audience_summary(theatre, implementation),
                        theatre.path.display()
                    ));
                }
                out.push_str(&format!("\n{} references", uses));
                if let Some(description) = &stage.description {
                    out.push_str("\n\n");
                    out.push_str(description);
                }
                if !stage.params.is_empty() {
                    out.push_str("\n\n| Parameter | Default |\n|---|---|\n");
                    for param in &stage.params {
                        out.push_str(&format!(
                            "| {} | {} |\n",
                            param.name,
                            param.default.as_ref().map_or_else(
                                || "required".to_string(),
                                |default| format!("`'{default}'`")
                            )
                        ));
                    }
                }
                out
            }
            SymbolKey::Theatre(name) => {
                let Some(theatre) = self.index.theatres.get(name) else {
                    return format!("**{name}**\n\nTheatre not found.");
                };
                let stages = self
                    .index
                    .stages
                    .values()
                    .filter(|stage| stage.theatre == theatre.name)
                    .map(|stage| stage.name.as_str())
                    .collect::<Vec<_>>();
                format!(
                    "**{}** — theatre\n\n{} stage(s): {}",
                    theatre.name,
                    stages.len(),
                    if stages.is_empty() {
                        "none".to_string()
                    } else {
                        stages.join(", ")
                    }
                )
            }
            SymbolKey::Play(name) => {
                let Some(play) = self
                    .workspace
                    .plays
                    .iter()
                    .find(|play| play.path.display().to_string() == *name)
                else {
                    return format!("**{name}**\n\nPlay not found.");
                };
                format!(
                    "**{}** — play\n\n{} act(s){}",
                    play.name,
                    play.acts.len(),
                    play.description
                        .as_ref()
                        .map_or_else(String::new, |description| format!("\n\n{description}"))
                )
            }
            SymbolKey::Act { play, name } => {
                let Some(act) = self.act(play, name) else {
                    return format!("**{name}**\n\nAct not found.");
                };
                let params = act
                    .params
                    .iter()
                    .map(|param| param.name.as_str())
                    .collect::<Vec<_>>();
                let tags = act
                    .tags
                    .iter()
                    .map(|tag| tag.name.as_str())
                    .collect::<Vec<_>>();
                format!(
                    "**{}** — act\n\nParameters: {}\n\nTags: {}\n\nPlatforms: {}",
                    act.name,
                    if params.is_empty() {
                        "none".to_string()
                    } else {
                        params.join(", ")
                    },
                    if tags.is_empty() {
                        "none".to_string()
                    } else {
                        tags.join(", ")
                    },
                    if act.platforms.is_empty() {
                        "all".to_string()
                    } else {
                        act.platforms
                            .iter()
                            .map(|p| p.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                )
            }
            SymbolKey::Var { play, name } => {
                let Some(play) = self.play_by_key(play) else {
                    return format!("**{name}** — variable");
                };
                let Some(var) = play.var_decls.iter().find(|var| var.name == *name) else {
                    return format!("**{name}** — variable");
                };
                format!(
                    "**{}** — variable\n\n```text\n{}\n```",
                    var.name, var.value.raw
                )
            }
            SymbolKey::Param { owner, name } => {
                let default = self
                    .index
                    .stages
                    .values()
                    .flat_map(|stage| &stage.params)
                    .find(|param| param.name == *name && owner.ends_with(&stage_owner_name(owner)))
                    .and_then(|param| param.default.clone());
                format!(
                    "**{}** — parameter\n\nOwner: `{}`\n\nDefault: {}",
                    name,
                    owner,
                    default.map_or_else(|| "required".to_string(), |value| format!("`'{value}'`"))
                )
            }
            SymbolKey::ExampleColumn { name, .. } => {
                format!("**{}** — example column", name)
            }
            SymbolKey::App(name) => {
                let app = self.apps.iter().find(|app| app.name == *name);
                format!(
                    "**{}** — app\n\n{}",
                    name,
                    app.and_then(|app| app.description.as_deref())
                        .unwrap_or("Configured app")
                )
            }
            SymbolKey::Actor(name) => self.actor_markdown(name, None),
            SymbolKey::Group(name) => self.group_markdown(name, None),
        }
    }

    fn actor_markdown(&self, name: &str, occurrence: Option<&Span>) -> String {
        let mut out = format!("**{name}** — actor\n\n");
        match self.devices.iter().find(|device| device.name == name) {
            Some(device) => out.push_str(&format!(
                "Configured device platform: `{}`.\n",
                device.platform.as_str()
            )),
            None => {
                out.push_str("Not configured in `impresario.devices.config` for this workspace.\n")
            }
        }
        let groups = self.groups_containing(name);
        if !groups.is_empty() {
            out.push_str(&format!(
                "\nGroups: {}\n",
                groups
                    .iter()
                    .map(|group| format!("`@{group}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(perform) = occurrence.and_then(|span| self.perform_at_filter_span(span)) {
            out.push_str(&self.stage_resolution_markdown(perform, name));
        }
        out
    }

    fn group_markdown(&self, name: &str, occurrence: Option<&Span>) -> String {
        let Some(members) = self.groups.get(name) else {
            return format!(
                "**@{name}** — actor group\n\nNot configured in `impresario.devices.config`."
            );
        };
        let resolved = self.expand_group(name);
        let direct = if members.is_empty() {
            "none".to_string()
        } else {
            members
                .iter()
                .map(|member| format!("`{member}`"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let devices = if resolved.is_empty() {
            "none".to_string()
        } else {
            resolved
                .iter()
                .map(|device| format!("`{device}`"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut out = format!(
            "**@{name}** — actor group\n\nDirect members: {direct}\n\nResolved devices: {devices}\n"
        );
        if let Some(perform) = occurrence.and_then(|span| self.perform_at_filter_span(span)) {
            out.push_str(&self.stage_resolution_markdown(perform, &format!("@{name}")));
        }
        out
    }

    fn groups_containing(&self, actor: &str) -> Vec<String> {
        self.groups
            .keys()
            .filter(|group| self.expand_group(group.as_str()).contains(actor))
            .cloned()
            .collect()
    }

    fn expand_group(&self, name: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        self.expand_group_into(name, &mut out, &mut Vec::new());
        out
    }

    fn expand_group_into(&self, name: &str, out: &mut BTreeSet<String>, stack: &mut Vec<String>) {
        if stack.iter().any(|item| item == name) {
            return;
        }
        let Some(members) = self.groups.get(name) else {
            return;
        };
        stack.push(name.to_string());
        for member in members {
            if let Some(group) = member.strip_prefix('@') {
                self.expand_group_into(group, out, stack);
            } else {
                out.insert(member.clone());
            }
        }
        stack.pop();
    }

    fn stage_resolution_markdown(&self, perform: &PerformDecl, reference: &str) -> String {
        let resolved = if let Some(group) = reference.strip_prefix('@') {
            self.expand_group(group)
        } else {
            BTreeSet::from([reference.to_string()])
        };
        let mut out = format!(
            "\n\nStage resolution for `Perform: {}`:\n\n\
             | Actor | Platform | Implementation | Audience |\n|---|---|---|---|\n",
            perform.stage_name
        );
        for actor in resolved {
            let Some(device) = self.devices.iter().find(|device| device.name == actor) else {
                out.push_str(&format!("| {actor} | unknown | not configured | - |\n"));
                continue;
            };
            let implementation = self
                .workspace
                .stage_instances(&perform.stage_name)
                .into_iter()
                .find(|&(theatre, stage)| theatre.stage_audience(stage).contains(&device.platform));
            match implementation {
                Some((theatre, stage)) => out.push_str(&format!(
                    "| {} | {} | `{}.{}` | {} |\n",
                    actor,
                    device.platform.as_str(),
                    theatre.name,
                    stage.name,
                    audience_summary(theatre, stage)
                )),
                None => out.push_str(&format!(
                    "| {} | {} | missing | - |\n",
                    actor,
                    device.platform.as_str()
                )),
            }
        }
        out
    }

    fn perform_at_filter_span(&self, span: &Span) -> Option<&PerformDecl> {
        for play in &self.workspace.plays {
            for act in &play.acts {
                for perform in &act.performs {
                    if filter_contains(perform, span) {
                        return Some(perform);
                    }
                }
            }
        }
        for theatre in &self.workspace.theatres {
            for stage in &theatre.stages {
                for item in &stage.items {
                    if let StageItem::Perform(perform) = item {
                        if filter_contains(perform, span) {
                            return Some(perform);
                        }
                    }
                }
            }
        }
        None
    }

    fn support_markdown(&self, verb: StepVerb) -> String {
        let mut out = match &verb {
            StepVerb::Action(verb) => format!("**{}** — action support\n\n", verb.name()),
            StepVerb::Check(condition) => {
                format!("**{}** — check support\n\n", condition.as_str())
            }
        };
        out.push_str("| Platform | Verdict |\n|---|---|\n");
        for platform in self.platforms() {
            let verdict = match &verb {
                StepVerb::Action(verb) => {
                    let verb = support::verb_of_action(verb);
                    let verdict = support::action_support(platform, verb);
                    if verb == Verb::Key {
                        "supported per key".to_string()
                    } else {
                        verdict.to_string()
                    }
                }
                StepVerb::Check(condition) => {
                    support::check_support(platform, support::verb_of_check(condition)).to_string()
                }
            };
            out.push_str(&format!("| {} | {} |\n", platform.as_str(), verdict));
        }
        if let StepVerb::Check(condition) = &verb {
            if matches!(
                condition,
                CheckCondition::Showing(_) | CheckCondition::Player(_)
            ) {
                out.push_str(
                    "\n| Platform | Probe | Evidence class | Expected confidence | Available sources |\n\
                     |---|---|---|---|---|\n",
                );
                for platform in self.platforms() {
                    let matrix = probe::support(platform);
                    let supported = match condition {
                        CheckCondition::Showing(_) => matrix.showing,
                        CheckCondition::Player(_) => matrix.player,
                        _ => false,
                    };
                    if !supported {
                        out.push_str(&format!(
                            "| {} | not supported | — | — | — |\n",
                            platform.as_str()
                        ));
                        continue;
                    }
                    let sources = matrix
                        .sources
                        .iter()
                        .filter(|source| source.available)
                        .map(|source| {
                            format!(
                                "{} ({})",
                                source.source.as_str(),
                                source.source.evidence_class().as_str()
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    out.push_str(&format!(
                        "| {} | supported | {} | {} | {} |\n",
                        platform.as_str(),
                        matrix.strongest_class().as_str(),
                        Confidence::Corroborated.as_str(),
                        sources
                    ));
                }
            }
        }
        out.push_str("\nUnsupported combinations are rejected by `IMP-DSL-013`.");
        out
    }

    /// Go-to-definition locations.
    #[must_use]
    pub fn definition(&self, path: &Path, position: Position) -> Vec<Span> {
        let Some(occurrence) = self.symbol_at(path, position) else {
            return Vec::new();
        };
        self.index.definitions(&occurrence.key)
    }

    /// Find-references locations.
    #[must_use]
    pub fn references(
        &self,
        path: &Path,
        position: Position,
        include_declaration: bool,
    ) -> Vec<Span> {
        let Some(occurrence) = self.symbol_at(path, position) else {
            return Vec::new();
        };
        self.index.references(&occurrence.key, include_declaration)
    }

    /// Prepare rename.
    #[must_use]
    pub fn prepare_rename(&self, path: &Path, position: Position) -> Option<PrepareRename> {
        let occurrence = self.symbol_at(path, position)?;
        if matches!(occurrence.key, SymbolKey::Play(_)) {
            return None;
        }
        Some(PrepareRename {
            span: occurrence.span.clone(),
            placeholder: self.index.key_name(&occurrence.key),
        })
    }

    /// Rename every occurrence workspace-wide.
    pub fn rename(
        &self,
        path: &Path,
        position: Position,
        new_name: &str,
    ) -> Result<WorkspaceEdit, String> {
        let occurrence = self
            .symbol_at(path, position)
            .ok_or_else(|| "no renameable symbol at this position".to_string())?;
        let key = occurrence.key.clone();
        match &key {
            SymbolKey::Play(_) => return Err("plays are renamed by their file".to_string()),
            SymbolKey::Actor(name) => return self.rename_actor_or_group(name, false, new_name),
            SymbolKey::Group(name) => return self.rename_actor_or_group(name, true, new_name),
            _ => {}
        }
        validate_name(new_name)?;
        let mut edit = WorkspaceEdit::default();
        let mut seen = BTreeSet::new();
        for span in self.index.references(&key, true) {
            if !seen.insert((span.file.clone(), span.line, span.col, span.len)) {
                continue;
            }
            edit.add(TextEdit {
                span,
                new_text: new_name.to_string(),
            });
        }
        Ok(edit)
    }

    /// Rename an actor (`@group` is a separate symbol) in every DSL reference
    /// and in the loaded devices-config layers: the device `name:` value, group
    /// keys and group membership references.
    fn rename_actor_or_group(
        &self,
        old_name: &str,
        is_group: bool,
        new_name: &str,
    ) -> Result<WorkspaceEdit, String> {
        let new_name = new_name.trim();
        let new_name = new_name.strip_prefix('@').unwrap_or(new_name);
        if !impresario_core::config::valid_device_name(new_name) {
            return Err("actor and group names must match ^[a-z0-9][a-z0-9_-]{0,62}$".to_string());
        }
        let mut edit = WorkspaceEdit::default();
        if new_name == old_name {
            return Ok(edit);
        }
        if self.device_configs.is_empty() {
            return Err(format!(
                "no devices configuration is loaded; cannot rename '{old_name}'"
            ));
        }

        let mut layers = Vec::new();
        let mut found = false;
        for source in &self.device_configs {
            let outcome = impresario_core::config::parse_devices(&source.text, &source.path);
            let Some(parsed) = outcome.config else {
                return Err(config_diagnostics_error(&source.path, &outcome.diagnostics));
            };
            for device in &parsed.devices {
                if device.name == new_name {
                    return Err(format!(
                        "device '{new_name}' already exists in {}",
                        source.path.display()
                    ));
                }
            }
            if parsed.groups.contains_key(new_name) {
                return Err(format!(
                    "group '{new_name}' already exists in {}",
                    source.path.display()
                ));
            }
            let layer = rename_layer(old_name, is_group, new_name, source)?;
            found |= layer.found;
            layers.push(layer);
        }
        let kind = if is_group { "group" } else { "actor" };
        if !found {
            return Err(format!(
                "{kind} '{old_name}' is not declared with an explicit name in the devices configuration"
            ));
        }

        let mut merged = impresario_core::config::DevicesConfig::default();
        for layer in &layers {
            let candidate = layer.candidate();
            let outcome = impresario_core::config::parse_devices(&candidate, &layer.path);
            let Some(parsed) = outcome.config else {
                return Err(config_diagnostics_error(&layer.path, &outcome.diagnostics));
            };
            merge_config(&mut merged, parsed);
        }
        if let Some((code, message)) = merged.validate_groups().into_iter().next() {
            return Err(format!("cannot rename: {}: {message}", code.as_str()));
        }
        let declared = if is_group {
            merged.groups.contains_key(new_name)
        } else {
            merged.devices.iter().any(|device| device.name == new_name)
        };
        if !declared {
            return Err(format!("cannot rename '{old_name}' to '{new_name}'"));
        }
        let still_declared = if is_group {
            merged.groups.contains_key(old_name)
        } else {
            merged.devices.iter().any(|device| device.name == old_name)
        };
        if still_declared {
            return Err(format!(
                "cannot rename '{old_name}': another config layer still declares that name"
            ));
        }

        let mut seen = BTreeSet::new();
        let key = if is_group {
            SymbolKey::Group(old_name.to_string())
        } else {
            SymbolKey::Actor(old_name.to_string())
        };
        let replacement = if is_group {
            format!("@{new_name}")
        } else {
            new_name.to_string()
        };
        for span in self.index.references(&key, true) {
            if !seen.insert((span.file.clone(), span.line, span.col, span.len)) {
                continue;
            }
            edit.add(TextEdit {
                span,
                new_text: replacement.clone(),
            });
        }
        for layer in layers {
            for text_edit in layer.edits {
                edit.add(text_edit);
            }
        }
        Ok(edit)
    }

    /// Document outline.
    #[must_use]
    pub fn document_symbols(&self, path: &Path) -> Vec<DocumentSymbol> {
        let source = self.source(path);
        if let Some(scene) = self.scene(path) {
            return vec![scene_symbol(scene, source)];
        }
        if let Some(theatre) = self.theatre(path) {
            return vec![theatre_symbol(theatre, source)];
        }
        if let Some(play) = self.play(path) {
            return vec![play_symbol(play, source)];
        }
        Vec::new()
    }

    /// Foldable indentation regions.
    #[must_use]
    pub fn folding_ranges(&self, path: &Path) -> Vec<FoldingRange> {
        let Some(text) = self.source(path) else {
            return Vec::new();
        };
        let scanned = scanner::scan(text, path);
        let lines = &scanned.lines;
        let mut ranges = Vec::new();
        for (index, line) in lines.iter().enumerate() {
            let next = lines.get(index + 1);
            if next.is_some_and(|next| next.depth > line.depth) {
                let end = lines
                    .iter()
                    .skip(index + 1)
                    .take_while(|candidate| candidate.depth > line.depth)
                    .last()
                    .map_or(line.line, |candidate| candidate.line);
                if end > line.line {
                    ranges.push(FoldingRange {
                        start_line: u32::try_from(line.line).unwrap_or(u32::MAX),
                        end_line: u32::try_from(end).unwrap_or(u32::MAX),
                        kind: Some("region".to_string()),
                    });
                }
            }
        }
        // Table blocks are contiguous pipe rows and fold as one region.
        let doc = Doc::new(text);
        let mut line = 1u32;
        while line <= doc.end_line() {
            if doc.line(line).trim_start().starts_with('|') {
                let start = line;
                while line < doc.end_line() && doc.line(line + 1).trim_start().starts_with('|') {
                    line += 1;
                }
                if line > start {
                    ranges.push(FoldingRange {
                        start_line: start,
                        end_line: line,
                        kind: Some("region".to_string()),
                    });
                }
            }
            line += 1;
        }
        ranges
    }

    /// Semantic tokens (keyword, namespace, function, property, variable).
    #[must_use]
    pub fn semantic_tokens(&self, path: &Path) -> Vec<SemanticToken> {
        let Some(text) = self.source(path) else {
            return Vec::new();
        };
        let mut tokens = Vec::new();
        for occurrence in &self.index.occurrences {
            if occurrence.span.file == path {
                tokens.push(SemanticToken {
                    span: occurrence.span.clone(),
                    token_type: Index::key_kind(&occurrence.key).to_string(),
                    modifiers: if occurrence.definition {
                        vec!["declaration".to_string()]
                    } else {
                        Vec::new()
                    },
                });
            }
        }
        let scanned = scanner::scan(text, path);
        for line in &scanned.lines {
            let trimmed = line.content.trim_start();
            let keyword = trimmed
                .split_once(':')
                .map(|(keyword, _)| keyword)
                .or_else(|| trimmed.split_once(' ').map(|(keyword, _)| keyword))
                .unwrap_or(trimmed);
            if keyword.is_empty() || keyword.starts_with("//") {
                continue;
            }
            let known = [
                "Play",
                "Theatre",
                "Scene",
                "Stage",
                "Act",
                "Description",
                "App",
                "Var",
                "Tags",
                "Platforms",
                "Platform",
                "Timeout",
                "Examples",
                "Perform",
                "Action",
                "Check",
                "is",
                "text",
                "textContains",
                "count",
                "app",
            ];
            if known.contains(&keyword) {
                let leading = line
                    .content
                    .chars()
                    .take_while(|ch| ch.is_whitespace())
                    .count();
                tokens.push(SemanticToken {
                    span: Span::new(path, line.line, line.col + leading, keyword.chars().count()),
                    token_type: "keyword".to_string(),
                    modifiers: Vec::new(),
                });
            }
        }
        tokens.sort_by(|left, right| {
            (left.span.line, left.span.col).cmp(&(right.span.line, right.span.col))
        });
        tokens.dedup_by(|left, right| left.span == right.span);
        // Remove tokens that start inside another token; LSP semantic tokens
        // must be non-overlapping.
        let mut filtered: Vec<SemanticToken> = Vec::new();
        for token in tokens {
            let overlaps = filtered.last().is_some_and(|last| {
                last.span.line == token.span.line
                    && token.span.col < last.span.col.saturating_add(last.span.len.max(1))
            });
            if !overlaps {
                filtered.push(token);
            }
        }
        filtered
    }

    /// Signature help for a `Perform: Stage(` argument list.
    #[must_use]
    pub fn signature_help(&self, path: &Path, position: Position) -> Option<SignatureHelp> {
        let prefix = self.prefix(path, position);
        let start = prefix.rfind("Perform:")?;
        let after = prefix.get(start + "Perform:".len()..)?.trim_start();
        let open = after.find('(')?;
        if after.get(open..)?.contains(')') {
            return None;
        }
        let stage_name = after.get(..open)?.trim();
        let stage = self.index.stage(stage_name)?;
        let label = stage.signature.clone();
        let parameters = stage
            .params
            .iter()
            .map(|param| SignatureParameter {
                label: match &param.default {
                    Some(default) => format!("{} = '{default}'", param.name),
                    None => param.name.clone(),
                },
                documentation: Some(match &param.default {
                    Some(default) => format!("default: '{default}'"),
                    None => "required".to_string(),
                }),
            })
            .collect();
        let arguments = after.get(open + 1..)?;
        let active = arguments.matches(',').count();
        Some(SignatureHelp {
            signatures: vec![Signature {
                label,
                documentation: stage.description.clone(),
                parameters,
            }],
            active_signature: 0,
            active_parameter: u32::try_from(active).unwrap_or(0),
        })
    }

    /// Default-argument inlay hints for `Perform:` calls.
    #[must_use]
    pub fn inlay_hints(&self, path: &Path, range: Option<(u32, u32)>) -> Vec<InlayHint> {
        let (start, end) = range.unwrap_or((1, u32::MAX));
        let mut hints = Vec::new();
        let Some(text) = self.source(path) else {
            return hints;
        };
        let doc = Doc::new(text);
        if let Some(play) = self.play(path) {
            for act in &play.acts {
                for perform in &act.performs {
                    self.perform_hints(perform, &doc, start, end, &mut hints);
                }
                for stage in &act.stages {
                    self.stage_hints(stage, &doc, start, end, &mut hints);
                }
            }
        }
        if let Some(theatre) = self.theatre(path) {
            for stage in &theatre.stages {
                self.stage_hints(stage, &doc, start, end, &mut hints);
            }
        }
        hints
    }

    fn perform_hints(
        &self,
        perform: &impresario_core::ast::PerformDecl,
        doc: &Doc<'_>,
        start: u32,
        end: u32,
        hints: &mut Vec<InlayHint>,
    ) {
        let line = u32::try_from(perform.span.line).unwrap_or(u32::MAX);
        if line < start || line > end {
            return;
        }
        let Some(stage) = self.index.stage(&perform.stage_name) else {
            return;
        };
        let provided = perform
            .args
            .iter()
            .map(|arg| arg.name.as_str())
            .collect::<BTreeSet<_>>();
        let missing = stage
            .params
            .iter()
            .filter(|param| !provided.contains(param.name.as_str()))
            .filter(|param| param.default.is_some())
            .collect::<Vec<_>>();
        if missing.is_empty() {
            return;
        }
        let col = u32::try_from(doc.line(line).chars().count()).unwrap_or(0) + 1;
        for param in missing {
            hints.push(InlayHint {
                span: Span::new(
                    doc_line_path(doc, line, perform.stage_span.file.clone()),
                    line as usize,
                    col as usize,
                    0,
                ),
                label: format!(
                    "{} = '{}'",
                    param.name,
                    param.default.as_deref().unwrap_or("")
                ),
                kind: "parameter".to_string(),
            });
        }
    }

    fn stage_hints(
        &self,
        stage: &StageDecl,
        doc: &Doc<'_>,
        start: u32,
        end: u32,
        hints: &mut Vec<InlayHint>,
    ) {
        for item in &stage.items {
            if let StageItem::Perform(perform) = item {
                self.perform_hints(perform, doc, start, end, hints);
            }
        }
        for block in &stage.scenes {
            let _ = block;
        }
    }

    /// Code lenses for runnable acts and reference counts.
    #[must_use]
    pub fn code_lenses(&self, path: &Path) -> Vec<CodeLens> {
        let mut lenses = Vec::new();
        if let Some(play) = self.play(path) {
            for act in &play.acts {
                let key = SymbolKey::Act {
                    play: path.display().to_string(),
                    name: act.name.clone(),
                };
                let count = self.index.references(&key, false).len();
                lenses.push(CodeLens {
                    span: act.name_span.clone(),
                    command: Command {
                        title: format!("Run act {}", act.name),
                        command: "impresario.runAct".to_string(),
                        arguments: vec![json!(path.display().to_string()), json!(act.name)],
                    },
                });
                lenses.push(CodeLens {
                    span: act.name_span.clone(),
                    command: Command {
                        title: format!("{count} reference(s)"),
                        command: "impresario.showReferences".to_string(),
                        arguments: vec![
                            json!(path.display().to_string()),
                            json!(act.name_span.line.saturating_sub(1)),
                            json!(act.name_span.col.saturating_sub(1)),
                        ],
                    },
                });
            }
        }
        if let Some(theatre) = self.theatre(path) {
            for stage in &theatre.stages {
                let key = SymbolKey::Stage(stage.name.clone());
                let count = self.index.references(&key, false).len();
                lenses.push(CodeLens {
                    span: stage.name_span.clone(),
                    command: Command {
                        title: format!("{count} reference(s)"),
                        command: "impresario.showReferences".to_string(),
                        arguments: vec![
                            json!(path.display().to_string()),
                            json!(stage.name_span.line.saturating_sub(1)),
                            json!(stage.name_span.col.saturating_sub(1)),
                        ],
                    },
                });
                let instances = self.workspace.stage_instances(&stage.name);
                let implementations = instances
                    .iter()
                    .map(|&(theatre, implementation)| {
                        json!({
                            "path": theatre.path.display().to_string(),
                            "line": implementation.name_span.line.saturating_sub(1),
                            "column": implementation.name_span.col.saturating_sub(1),
                            "theatre": theatre.name.as_str(),
                            "audience": audience_summary(theatre, implementation),
                        })
                    })
                    .collect::<Vec<_>>();
                let platforms = instances
                    .iter()
                    .flat_map(|&(theatre, implementation)| theatre.stage_audience(implementation))
                    .collect::<BTreeSet<_>>();
                let arguments = vec![json!(stage.name.as_str()), json!(implementations)];
                lenses.push(CodeLens {
                    span: stage.name_span.clone(),
                    command: Command {
                        title: format!("{} implementation(s)", instances.len()),
                        command: "impresario.showStageImplementations".to_string(),
                        arguments: arguments.clone(),
                    },
                });
                lenses.push(CodeLens {
                    span: stage.name_span.clone(),
                    command: Command {
                        title: format!(
                            "audience coverage: {}/{} platforms",
                            platforms.len(),
                            Platform::ALL.len()
                        ),
                        command: "impresario.showStageImplementations".to_string(),
                        arguments,
                    },
                });
            }
        }
        if let Some(scene) = self.scene(path) {
            for piece in &scene.pieces {
                let key = SymbolKey::Piece {
                    scene: scene.name.clone(),
                    name: piece.name.clone(),
                };
                let count = self.index.references(&key, false).len();
                lenses.push(CodeLens {
                    span: piece.name_span.clone(),
                    command: Command {
                        title: format!("{count} reference(s)"),
                        command: "impresario.showReferences".to_string(),
                        arguments: vec![
                            json!(path.display().to_string()),
                            json!(piece.name_span.line.saturating_sub(1)),
                            json!(piece.name_span.col.saturating_sub(1)),
                        ],
                    },
                });
            }
        }
        lenses
    }

    fn act(&self, play_key: &str, name: &str) -> Option<&impresario_core::ast::ActDecl> {
        self.play_by_key(play_key)?
            .acts
            .iter()
            .find(|act| act.name == name)
    }

    fn play_by_key(&self, key: &str) -> Option<&PlayFile> {
        if key == "*" {
            return self.workspace.plays.first();
        }
        self.workspace
            .plays
            .iter()
            .find(|play| play.path.display().to_string() == key)
    }

    fn verb_at(&self, path: &Path, line: u32) -> Option<(StepVerb, Span)> {
        let line = line as usize;
        for play in &self.workspace.plays {
            if play.path != path {
                continue;
            }
            for act in &play.acts {
                if let Some(found) = action_in_play(act, line) {
                    return Some(found);
                }
            }
        }
        for theatre in &self.workspace.theatres {
            if theatre.path != path {
                continue;
            }
            for stage in &theatre.stages {
                if let Some(found) = action_in_stage(stage, line) {
                    return Some(found);
                }
            }
        }
        None
    }
}

fn audience_summary(theatre: &TheatreFile, stage: &StageDecl) -> String {
    let audience = theatre.stage_audience(stage);
    if audience.is_empty() {
        "none".to_string()
    } else {
        audience
            .iter()
            .map(|platform| platform.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn filter_contains(perform: &PerformDecl, span: &Span) -> bool {
    perform
        .filter
        .as_ref()
        .is_some_and(|filter| filter.actors.iter().any(|actor| actor.span == *span))
}

fn doc_line_path(_doc: &Doc<'_>, _line: u32, path: PathBuf) -> PathBuf {
    path
}

enum StepVerb {
    Action(ActionVerb),
    Check(CheckCondition),
}

fn action_in_play(act: &impresario_core::ast::ActDecl, line: usize) -> Option<(StepVerb, Span)> {
    for stage in &act.stages {
        if let Some(found) = action_in_stage(stage, line) {
            return Some(found);
        }
    }
    None
}

fn action_in_stage(stage: &StageDecl, line: usize) -> Option<(StepVerb, Span)> {
    for item in &stage.items {
        match item {
            StageItem::Scene(block) => {
                for step in &block.steps {
                    if step.span.line == line {
                        return match &step.kind {
                            impresario_core::ast::StepKind::Action(action) => {
                                Some((StepVerb::Action(action.verb.clone()), action.span.clone()))
                            }
                            impresario_core::ast::StepKind::Check(check) => {
                                Some((StepVerb::Check(check.condition.clone()), check.span.clone()))
                            }
                        };
                    }
                }
            }
            StageItem::Perform(_) => {}
        }
    }
    for block in &stage.scenes {
        for step in &block.steps {
            if step.span.line == line {
                return match &step.kind {
                    impresario_core::ast::StepKind::Action(action) => {
                        Some((StepVerb::Action(action.verb.clone()), action.span.clone()))
                    }
                    impresario_core::ast::StepKind::Check(check) => {
                        Some((StepVerb::Check(check.condition.clone()), check.span.clone()))
                    }
                };
            }
        }
    }
    None
}

/// A full-line-block span. Document symbols need a range that contains their
/// selection range; core `Span::len` is scalar text length, so walk every line
/// from column 1 through the end of `end_line`.
fn block_span(source: Option<&str>, name_span: &Span, end_line: usize) -> Span {
    let start_line = name_span.line.max(1);
    let end_line = end_line.max(start_line);
    let Some(source) = source else {
        // Parsed symbols always have a source; this fallback still contains
        // the name if one is missing (for example a synthesized workspace).
        return Span::new(
            name_span.file.clone(),
            start_line,
            1,
            name_span
                .col
                .saturating_add(name_span.len)
                .saturating_sub(1),
        );
    };
    let lines: Vec<&str> = source.split('\n').collect();
    let mut length = 0usize;
    for line in start_line..=end_line {
        let raw = lines.get(line.saturating_sub(1)).copied().unwrap_or("");
        let content = raw.strip_suffix('\r').unwrap_or(raw);
        length = length.saturating_add(content.chars().count());
        if line < end_line {
            length = length.saturating_add(1);
        }
    }
    Span::new(name_span.file.clone(), start_line, 1, length)
}

fn scene_block_end_line(block: &SceneBlock) -> usize {
    block
        .steps
        .last()
        .map_or(block.scene_span.line, |step| step.span.line)
}

fn stage_end_line(stage: &StageDecl) -> usize {
    let mut end = stage.name_span.line;
    for item in &stage.items {
        end = end.max(match item {
            StageItem::Scene(block) => scene_block_end_line(block),
            StageItem::Perform(perform) => perform.span.line,
        });
    }
    for block in &stage.scenes {
        end = end.max(scene_block_end_line(block));
    }
    end
}

fn act_end_line(act: &ActDecl) -> usize {
    let mut end = act.name_span.line;
    for perform in &act.performs {
        end = end.max(perform.span.line);
    }
    for stage in &act.stages {
        end = end.max(stage_end_line(stage));
    }
    if let Some(examples) = &act.examples {
        if let Some(row) = examples.rows.last() {
            end = end.max(row.span.line);
        }
    }
    end
}

fn scene_symbol(scene: &SceneFile, source: Option<&str>) -> DocumentSymbol {
    let end_line = scene.pieces.last().map_or(scene.name_span.line, |piece| {
        piece.span.line.max(scene.platforms_span.line)
    });
    DocumentSymbol {
        name: scene.name.clone(),
        detail: Some(match &scene.platforms {
            impresario_core::platforms::ScenePlatforms::CrossPlatform => {
                "cross-platform".to_string()
            }
            impresario_core::platforms::ScenePlatforms::List(list) => list
                .iter()
                .map(|platform| platform.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        }),
        kind: "namespace".to_string(),
        span: block_span(source, &scene.name_span, end_line),
        selection_span: scene.name_span.clone(),
        children: scene
            .pieces
            .iter()
            .map(|piece| DocumentSymbol {
                name: piece.name.clone(),
                detail: Some(crate::model::selectors_summary(&piece.selectors)),
                kind: "property".to_string(),
                span: piece.span.clone(),
                selection_span: piece.name_span.clone(),
                children: Vec::new(),
            })
            .collect(),
    }
}

fn theatre_symbol(theatre: &TheatreFile, source: Option<&str>) -> DocumentSymbol {
    let end_line = theatre
        .stages
        .last()
        .map_or(theatre.name_span.line, stage_end_line);
    DocumentSymbol {
        name: theatre.name.clone(),
        detail: Some("theatre".to_string()),
        kind: "namespace".to_string(),
        span: block_span(source, &theatre.name_span, end_line),
        selection_span: theatre.name_span.clone(),
        children: theatre
            .stages
            .iter()
            .map(|stage| stage_symbol(stage, source))
            .collect(),
    }
}

fn play_symbol(play: &PlayFile, source: Option<&str>) -> DocumentSymbol {
    let end_line = play.acts.last().map_or(play.name_span.line, act_end_line);
    DocumentSymbol {
        name: play.name.clone(),
        detail: play.description.clone(),
        kind: "namespace".to_string(),
        span: block_span(source, &play.name_span, end_line),
        selection_span: play.name_span.clone(),
        children: play
            .acts
            .iter()
            .map(|act| DocumentSymbol {
                name: act.name.clone(),
                detail: Some(format!(
                    "{} parameter(s), {} tag(s)",
                    act.params.len(),
                    act.tags.len()
                )),
                kind: "class".to_string(),
                span: block_span(source, &act.name_span, act_end_line(act)),
                selection_span: act.name_span.clone(),
                children: act
                    .performs
                    .iter()
                    .map(|perform| DocumentSymbol {
                        name: perform.stage_name.clone(),
                        detail: Some("Perform".to_string()),
                        kind: "function".to_string(),
                        span: perform.stage_span.clone(),
                        selection_span: perform.stage_span.clone(),
                        children: Vec::new(),
                    })
                    .chain(act.stages.iter().map(|stage| stage_symbol(stage, source)))
                    .collect(),
            })
            .collect(),
    }
}

fn stage_symbol(stage: &StageDecl, source: Option<&str>) -> DocumentSymbol {
    DocumentSymbol {
        name: stage.name.clone(),
        detail: Some("Stage".to_string()),
        kind: "function".to_string(),
        span: block_span(source, &stage.name_span, stage_end_line(stage)),
        selection_span: stage.name_span.clone(),
        children: stage
            .scenes
            .iter()
            .map(|block| DocumentSymbol {
                name: block.scene_name.clone(),
                detail: Some("Scene".to_string()),
                kind: "namespace".to_string(),
                span: block_span(source, &block.scene_span, scene_block_end_line(block)),
                selection_span: block.scene_span.clone(),
                children: Vec::new(),
            })
            .collect::<Vec<_>>(),
    }
}

fn stage_owner_name(owner: &str) -> &str {
    owner.strip_prefix("stage:").unwrap_or(owner)
}

fn resolve_selector(
    piece: &crate::model::PieceInfo,
    platform: Platform,
) -> Option<(&impresario_core::ast::PieceSelector, Platform)> {
    for key in platform.selector_chain() {
        if let Some(selector) = piece.selectors.iter().find(|selector| selector.key == *key) {
            return Some((selector, *key));
        }
    }
    None
}

/// One devices-config layer's pending rename edits.
struct RenameLayer {
    path: PathBuf,
    source: String,
    edits: Vec<TextEdit>,
    raw: Vec<(usize, usize, String)>,
    found: bool,
}

impl RenameLayer {
    fn push(&mut self, path: &Path, text: &str, scalar: &yaml_edit::Scalar, replacement: String) {
        let range = scalar.byte_range();
        let start = range.start as usize;
        let end = range.end as usize;
        self.raw.push((start, end, replacement.clone()));
        self.edits.push(TextEdit {
            span: span_of_bytes(path, text, start, end),
            new_text: replacement,
        });
    }

    /// The layer text after every pending edit, used to validate the
    /// prospective configuration before returning the workspace edit.
    fn candidate(&self) -> String {
        let mut text = self.source.clone();
        let mut raw = self.raw.clone();
        raw.sort_by(|left, right| right.0.cmp(&left.0));
        for (start, end, replacement) in raw {
            text.replace_range(start..end, &replacement);
        }
        text
    }
}

fn rename_layer(
    old_name: &str,
    is_group: bool,
    new_name: &str,
    source: &DeviceConfigSource,
) -> Result<RenameLayer, String> {
    let document = Document::from_str(&source.text)
        .map_err(|error| format!("cannot parse {}: {error}", source.path.display()))?;
    let mut layer = RenameLayer {
        path: source.path.clone(),
        source: source.text.clone(),
        edits: Vec::new(),
        raw: Vec::new(),
        found: false,
    };
    let devices_node = document.get("devices");
    if let Some(devices) = devices_node.as_ref().and_then(|node| node.as_sequence()) {
        for device in devices.values() {
            let Some(mapping) = device.as_mapping() else {
                continue;
            };
            let Some(name_node) = mapping.get("name") else {
                continue;
            };
            let Some(name) = name_node.as_scalar() else {
                continue;
            };
            if !is_group && name.as_string() == old_name {
                layer.found = true;
                layer.push(&source.path, &source.text, name, new_name.to_string());
            }
        }
    }
    if let Some(groups) = document.get_mapping("groups") {
        for (key_node, members_node) in groups.iter() {
            let Some(key) = key_node.as_scalar() else {
                continue;
            };
            if is_group && key.as_string() == old_name {
                layer.found = true;
                layer.push(&source.path, &source.text, key, new_name.to_string());
            }
            let Some(members) = members_node.as_sequence() else {
                continue;
            };
            for member_node in members.values() {
                let Some(member) = member_node.as_scalar() else {
                    continue;
                };
                let value = member.as_string();
                let replace = if is_group {
                    value.strip_prefix('@') == Some(old_name)
                } else {
                    value == old_name
                };
                if replace {
                    let replacement = if is_group {
                        // `@` is a YAML reserved indicator, so a group
                        // reference must stay quoted.
                        format!("\"@{new_name}\"")
                    } else {
                        new_name.to_string()
                    };
                    layer.push(&source.path, &source.text, member, replacement);
                }
            }
        }
    }
    Ok(layer)
}

fn merge_config(
    merged: &mut impresario_core::config::DevicesConfig,
    layer: impresario_core::config::DevicesConfig,
) {
    for device in layer.devices {
        if let Some(existing) = merged
            .devices
            .iter_mut()
            .find(|candidate| candidate.name == device.name)
        {
            *existing = device;
        } else {
            merged.devices.push(device);
        }
    }
    for (name, members) in layer.groups {
        merged.groups.insert(name, members);
    }
}

fn config_diagnostics_error(
    path: &Path,
    diagnostics: &impresario_core::diag::Diagnostics,
) -> String {
    let messages = diagnostics
        .iter()
        .map(|diagnostic| format!("{}: {}", diagnostic.code.as_str(), diagnostic.message))
        .collect::<Vec<_>>()
        .join("; ");
    if messages.is_empty() {
        format!("cannot parse {}", path.display())
    } else {
        format!("cannot rename in {}: {messages}", path.display())
    }
}

fn span_of_bytes(path: &Path, text: &str, start: usize, end: usize) -> Span {
    let before = &text[..start];
    let line = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |index| index + 1);
    let col = text[line_start..start].chars().count() + 1;
    let len = text[start..end].chars().count();
    Span::new(path, line, col, len)
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("the new name is empty".to_string());
    }
    if name.len() > 200 {
        return Err("the new name is longer than 200 characters".to_string());
    }
    if name.chars().any(|ch| {
        ch.is_whitespace()
            || matches!(
                ch,
                '"' | '\'' | ',' | '(' | ')' | '|' | ':' | '$' | '{' | '}'
            )
    }) {
        return Err(
            "the new name contains a character that is not allowed in a DSL name".to_string(),
        );
    }
    Ok(())
}
