//! File parsers: hand-written structure (indentation, headers, recovery)
//! over the winnow line grammars in [`grammar`].
//!
//! Rules implemented here (spec 5.2, 5.4, 5.5): recovering (collect as many
//! diagnostics as possible), never panics, one `Step` per Action/Check line.

mod grammar;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use crate::ast::{
    ActDecl, Action, ActionVerb, AppOp, Check, CheckCondition, Direction, Ident, KeyName, Named,
    PieceDecl, PieceSelector, PlayFile, SceneBlock, SceneFile, StageDecl, Step, StepKind, VarDecl,
};
use crate::diag::{suggest, Code, Diagnostic, Diagnostics};
use crate::duration::DslDuration;
use crate::keys::RemoteKey;
use crate::platforms::{Platform, ScenePlatforms};
use crate::span::Span;
use crate::strings::{self, ParsedString};

/// Result of parsing one file.
#[derive(Debug)]
pub struct ParseOutcome<T> {
    pub ast: Option<T>,
    pub diagnostics: Diagnostics,
}

#[derive(Debug, Clone)]
struct Cursor<'a> {
    start: &'a str,
    rest: &'a str,
    file: &'a Path,
    line: usize,
    base_col: usize,
}

impl<'a> Cursor<'a> {
    fn new(input: &'a str, file: &'a Path, line: usize, base_col: usize) -> Self {
        Self {
            start: input,
            rest: input,
            file,
            line,
            base_col,
        }
    }

    /// 1-based column of the cursor.
    fn col(&self) -> usize {
        let consumed_bytes = self.start.len().saturating_sub(self.rest.len());
        let chars = self
            .start
            .get(..consumed_bytes)
            .map(|prefix| prefix.chars().count())
            .unwrap_or(0);
        self.base_col + chars
    }

    fn span(&self, col: usize, len: usize) -> Span {
        Span::new(self.file, self.line, col, len)
    }

    fn skip_ws(&mut self) {
        self.rest = self.rest.trim_start_matches([' ', '\t']);
    }

    fn at_end(&mut self) -> bool {
        self.skip_ws();
        self.rest.is_empty()
    }

    fn try_lit(&mut self, literal: &str) -> bool {
        self.skip_ws();
        if let Some(rest) = self.rest.strip_prefix(literal) {
            self.rest = rest;
            true
        } else {
            false
        }
    }

    fn expect_lit(&mut self, literal: &str) -> Result<(), String> {
        if self.try_lit(literal) {
            Ok(())
        } else {
            Err(format!("expected '{literal}'"))
        }
    }

    fn ident(&mut self) -> Result<Ident, String> {
        self.skip_ws();
        let col = self.col();
        let mut input = self.rest;
        match grammar::ident.parse_next(&mut input) {
            Ok(name) => {
                self.rest = input;
                Ok(Ident {
                    span: self.span(col, name.chars().count()),
                    name,
                })
            }
            Err(_) => Err("expected an identifier".to_string()),
        }
    }

    fn word(&mut self) -> Result<(&'a str, Span), String> {
        self.skip_ws();
        let col = self.col();
        let mut input = self.rest;
        match grammar::token.parse_next(&mut input) {
            Ok(word) => {
                self.rest = input;
                Ok((word, self.span(col, word.chars().count())))
            }
            Err(_) => Err("expected a word".to_string()),
        }
    }

    fn uint(&mut self) -> Result<u32, String> {
        self.skip_ws();
        let mut input = self.rest;
        match grammar::integer.parse_next(&mut input) {
            Ok(value) => {
                self.rest = input;
                Ok(value)
            }
            Err(_) => Err("expected an integer".to_string()),
        }
    }

    fn quoted(&mut self) -> Result<ParsedString, String> {
        self.skip_ws();
        let col = self.col();
        match strings::parse_quoted(self.rest, self.file, self.line, col) {
            Ok((parsed, consumed)) => {
                let byte_len: usize = self
                    .rest
                    .char_indices()
                    .take(consumed)
                    .last()
                    .map(|(idx, c)| idx + c.len_utf8())
                    .unwrap_or(0);
                self.rest = self.rest.get(byte_len..).unwrap_or("");
                Ok(parsed)
            }
            Err(diag) => Err(diag.message),
        }
    }

    fn quoted_dq(&mut self) -> Result<ParsedString, String> {
        self.skip_ws();
        if !self.rest.starts_with('"') {
            return Err("selectors are double-quoted strings".to_string());
        }
        self.quoted()
    }

    /// A direction word via the winnow grammar.
    fn direction(&mut self) -> Result<Direction, String> {
        self.skip_ws();
        let mut input = self.rest;
        match grammar::direction.parse_next(&mut input) {
            Ok(direction) => {
                self.rest = input;
                Ok(direction)
            }
            Err(_) => Err(self.peek_word()),
        }
    }

    /// A remote key name via the winnow grammar.
    fn keyname(&mut self) -> Result<(RemoteKey, &'a str, Span), String> {
        self.skip_ws();
        let col = self.col();
        let mut input = self.rest;
        match grammar::keyname.parse_next(&mut input) {
            Ok(key) => {
                let consumed = self.rest.len() - input.len();
                let name = self.rest.get(..consumed).unwrap_or("");
                self.rest = input;
                Ok((key, name, self.span(col, name.chars().count())))
            }
            Err(_) => Err(self.peek_word()),
        }
    }

    /// The next word without consuming it (for diagnostics).
    fn peek_word(&self) -> String {
        self.rest
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string()
    }

    /// A duration literal: digits + unit.
    fn duration(&mut self) -> Result<DslDuration, String> {
        self.skip_ws();
        let col = self.col();
        let (word, _) = self.word()?;
        let looks_like_duration = word.chars().next().is_some_and(|c| c.is_ascii_digit());
        if !looks_like_duration {
            return Err(format!("expected a duration, found '{word}'"));
        }
        let span = self.span(col, word.chars().count());
        match crate::duration::parse(word, Some(span)) {
            Ok(duration) => Ok(duration),
            Err(diag) => Err(diag.message),
        }
    }
}

// Silence an unused-parameter warning helper: the hint is intentionally part
// of the error flow above but not required for correctness.

use winnow::Parser;

fn diag_at(code: Code, message: impl Into<String>, span: Span) -> Diagnostic {
    Diagnostic::error(code, message).with_span(span)
}

// ---------------------------------------------------------------------------
// Scene files
// ---------------------------------------------------------------------------

/// Parse a `.scene` file.
#[must_use]
pub fn parse_scene(source: &str, path: &Path) -> ParseOutcome<SceneFile> {
    let scanned = crate::scanner::scan(source, path);
    let mut diagnostics = Diagnostics::new();
    diagnostics.extend(scanned.diagnostics);
    let lines = scanned.lines;

    let mut iter = lines.iter().enumerate();
    let mut name = None;
    let mut name_span = None;
    let mut platforms = None;
    let mut platforms_span = None;

    // Header line 1: Scene:
    match iter.next() {
        Some((_, line)) if line.depth == 0 && line.content.starts_with("Scene:") => {
            let body_col = line.col + "Scene:".chars().count();
            let body = line.content.get("Scene:".len()..).unwrap_or("").trim();
            let mut cursor =
                Cursor::new(body, path, line.line, body_col + leading_ws(line, "Scene:"));
            match cursor.ident() {
                Ok(ident) => {
                    if !cursor.at_end() {
                        diagnostics.push(diag_at(
                            Code::DslUnknownKeyword,
                            "unexpected input after the scene name",
                            cursor.span(cursor.col(), 1),
                        ));
                    }
                    name = Some(ident.name);
                    name_span = Some(ident.span);
                }
                Err(reason) => diagnostics.push(diag_at(
                    Code::DslMissingHeader,
                    format!("Scene: needs a name ({reason})"),
                    line.span(path),
                )),
            }
        }
        Some((_, line)) => diagnostics.push(diag_at(
            Code::DslMissingHeader,
            "a scene file must start with 'Scene: <Name>'",
            line.span(path),
        )),
        None => {
            diagnostics.push(Diagnostic::error(
                Code::DslMissingHeader,
                "a scene file must start with 'Scene: <Name>'",
            ));
            return ParseOutcome {
                ast: None,
                diagnostics,
            };
        }
    }

    // Header line 2: Platform:
    match iter.next() {
        Some((_, line)) if line.depth == 0 && line.content.starts_with("Platform:") => {
            let body_col = line.col + "Platform:".chars().count();
            let body = line.content.get("Platform:".len()..).unwrap_or("").trim();
            let body_col = body_col + leading_ws(line, "Platform:");
            let span = Span::new(path, line.line, body_col, body.chars().count());
            match parse_platform_list(body) {
                Ok(list) => {
                    platforms = Some(list);
                    platforms_span = Some(span);
                }
                Err(reason) => diagnostics.push(diag_at(Code::DslUnknownKeyword, reason, span)),
            }
        }
        Some((_, line)) => diagnostics.push(diag_at(
            Code::DslMissingHeader,
            "a scene file needs 'Platform: ...' on its second line",
            line.span(path),
        )),
        None => diagnostics.push(Diagnostic::error(
            Code::DslMissingHeader,
            "a scene file needs 'Platform: ...' on its second line",
        )),
    }

    // Header line 3: Pieces:
    match iter.next() {
        Some((_, line)) if line.depth == 0 && line.content.trim() == "Pieces:" => {}
        Some((_, line)) => diagnostics.push(diag_at(
            Code::DslMissingHeader,
            "expected 'Pieces:' after the Platform line",
            line.span(path),
        )),
        None => diagnostics.push(Diagnostic::error(
            Code::DslMissingHeader,
            "expected 'Pieces:' with at least one piece",
        )),
    }

    let mut pieces: Vec<PieceDecl> = Vec::new();
    for (_, line) in iter {
        if line.depth != 1 {
            let code = if line.depth == 0 {
                Code::DslUnknownKeyword
            } else {
                Code::DslMissingHeader
            };
            diagnostics.push(diag_at(
                code,
                "piece declarations must be indented one level under 'Pieces:'",
                line.span(path),
            ));
            continue;
        }
        match parse_piece_line(line, path) {
            Ok(piece) => {
                if let Some(previous) = pieces.iter().find(|p| p.name == piece.name) {
                    diagnostics.push(
                        Diagnostic::error(
                            Code::DslDuplicatePiece,
                            format!("piece '{}' is declared twice", piece.name),
                        )
                        .with_span(piece.name_span.clone())
                        .with_help(format!(
                            "first declared at line {}",
                            previous.name_span.line
                        )),
                    );
                    continue;
                }
                pieces.push(piece);
            }
            Err(diag) => diagnostics.push(diag),
        }
    }

    if pieces.is_empty() && diagnostics.errors().count() == 0 {
        diagnostics.push(Diagnostic::error(
            Code::DslMissingHeader,
            "a scene needs at least one piece",
        ));
    }

    let ast = match (name, name_span, platforms, platforms_span) {
        (Some(name), Some(name_span), Some(platforms), Some(platforms_span)) => Some(SceneFile {
            path: PathBuf::from(path),
            name,
            name_span,
            platforms,
            platforms_span,
            pieces,
        }),
        _ => None,
    };

    ParseOutcome { ast, diagnostics }
}

fn leading_ws(line: &crate::scanner::LogicalLine, keyword: &str) -> usize {
    line.content
        .get(keyword.len()..)
        .map(|rest| rest.len() - rest.trim_start().len())
        .unwrap_or(0)
}

fn parse_platform_list(body: &str) -> Result<ScenePlatforms, String> {
    if body == "cross-platform" {
        return Ok(ScenePlatforms::CrossPlatform);
    }
    let mut rest = body;
    let mut list: Vec<Platform> = Vec::new();
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        match grammar::platform.parse_next(&mut rest) {
            Ok(platform) => {
                if !list.contains(&platform) {
                    list.push(platform);
                }
            }
            Err(_) => {
                let word = rest.split(',').next().unwrap_or("").trim();
                let names: Vec<&str> = Platform::ALL.iter().map(|p| p.as_str()).collect();
                let hint = suggest(word, &names)
                    .map(|h| format!("; {h}"))
                    .unwrap_or_default();
                return Err(format!("unknown platform '{word}' in Platform: list{hint}"));
            }
        }
        rest = rest.trim_start();
        if let Some(remainder) = rest.strip_prefix(',') {
            rest = remainder;
        } else if rest.is_empty() {
            break;
        } else {
            return Err(format!("expected ',' between platforms, found '{rest}'"));
        }
    }
    if list.is_empty() {
        return Err("Platform: needs 'cross-platform' or a platform list".to_string());
    }
    Ok(ScenePlatforms::List(list))
}

fn parse_piece_line(
    line: &crate::scanner::LogicalLine,
    path: &Path,
) -> Result<PieceDecl, Diagnostic> {
    let mut cursor = Cursor::new(&line.content, path, line.line, line.col);
    let name = cursor.ident().map_err(|reason| {
        diag_at(
            Code::DslUnknownKeyword,
            format!("piece line must start with a name ({reason})"),
            line.span(path),
        )
    })?;
    cursor.expect_lit(":").map_err(|_| {
        diag_at(
            Code::DslUnknownKeyword,
            "expected ':' after the piece name",
            name.span.clone(),
        )
    })?;

    let mut selectors: Vec<PieceSelector> = Vec::new();
    let mut index = 0u32;
    let mut index_seen = false;

    loop {
        cursor.skip_ws();
        let mut probe = cursor.clone();
        if probe.try_lit("index") && probe.try_lit("=") {
            let col = probe.col();
            let value = probe.uint().map_err(|_| {
                diag_at(
                    Code::DslInvalidSelector,
                    "index must be a non-negative integer",
                    probe.span(col, 1),
                )
            })?;
            if index_seen {
                return Err(diag_at(
                    Code::DslInvalidSelector,
                    "index is given twice in one piece",
                    cursor.span(col, 5),
                ));
            }
            index = value;
            index_seen = true;
            cursor = probe;
        } else {
            let (word, span) = cursor.word().map_err(|_| {
                diag_at(
                    Code::DslInvalidSelector,
                    "expected 'platform=\"selector\"'",
                    cursor.span(cursor.col(), 1),
                )
            })?;
            let key = word.parse::<Platform>().map_err(|()| {
                let names: Vec<&str> = Platform::ALL.iter().map(|p| p.as_str()).collect();
                let hint = suggest(word, &names)
                    .map(|h| format!("; {h}"))
                    .unwrap_or_default();
                diag_at(
                    Code::DslInvalidSelector,
                    format!("unknown selector key '{word}'{hint}"),
                    span.clone(),
                )
            })?;
            if selectors.iter().any(|s| s.key == key) {
                return Err(diag_at(
                    Code::DslInvalidSelector,
                    format!("selector key '{key}' is given twice"),
                    span,
                ));
            }
            cursor.expect_lit("=").map_err(|_| {
                diag_at(
                    Code::DslInvalidSelector,
                    format!("expected '=' after selector key '{key}'"),
                    cursor.span(cursor.col(), 1),
                )
            })?;
            let literal_col = cursor.col();
            let parsed = cursor.quoted_dq().map_err(|reason| {
                diag_at(
                    Code::DslInvalidSelector,
                    format!("selector for '{key}' must be a double-quoted string ({reason})"),
                    cursor.span(literal_col, 1),
                )
            })?;
            selectors.push(PieceSelector {
                key,
                raw: parsed.raw,
                span: parsed.span,
            });
        }

        cursor.skip_ws();
        if cursor.try_lit(",") {
            continue;
        }
        break;
    }

    if !cursor.at_end() {
        return Err(diag_at(
            Code::DslInvalidSelector,
            format!(
                "unexpected trailing input '{}' in piece line",
                cursor.rest.trim()
            ),
            cursor.span(cursor.col(), cursor.rest.trim().chars().count().max(1)),
        ));
    }
    if selectors.is_empty() {
        return Err(diag_at(
            Code::DslInvalidSelector,
            format!("piece '{}' needs at least one selector", name.name),
            name.span.clone(),
        ));
    }

    Ok(PieceDecl {
        name: name.name,
        name_span: name.span,
        span: line.span(path),
        selectors,
        index,
    })
}

// ---------------------------------------------------------------------------
// Play files
// ---------------------------------------------------------------------------

const PLAY_KEYWORDS: &[&str] = &[
    "Play",
    "Description",
    "App",
    "Var",
    "Act",
    "Tags",
    "Platforms",
    "Stage",
    "Timeout",
    "Scene",
    "Action",
    "Check",
    "Pieces",
    "Platform",
];

/// Parse a `.play` file.
#[must_use]
pub fn parse_play(source: &str, path: &Path) -> ParseOutcome<PlayFile> {
    let scanned = crate::scanner::scan(source, path);
    let mut diagnostics = Diagnostics::new();
    diagnostics.extend(scanned.diagnostics);
    let lines = scanned.lines;

    let mut name: Option<String> = None;
    let mut name_span: Option<Span> = None;
    let mut description: Option<String> = None;
    let mut app: Option<Named> = None;
    let mut var_decls: Vec<VarDecl> = Vec::new();
    let mut acts: Vec<ActDecl> = Vec::new();
    let mut i = 0usize;

    // Play:
    match lines.first() {
        Some(line) if line.depth == 0 && line.content.starts_with("Play:") => {
            match header_text(line, "Play", path) {
                Ok((text, span)) => {
                    if strings::free_text_ok(&text) {
                        name = Some(text);
                        name_span = Some(span);
                    } else {
                        diagnostics.push(diag_at(
                            Code::DslMissingHeader,
                            "Play: name must be 1..=200 characters",
                            span,
                        ));
                    }
                }
                Err(diag) => diagnostics.push(diag),
            }
            i = 1;
        }
        Some(line) => diagnostics.push(diag_at(
            Code::DslMissingHeader,
            "a play file must start with 'Play: <name>'",
            line.span(path),
        )),
        None => {
            diagnostics.push(Diagnostic::error(
                Code::DslMissingHeader,
                "a play file must start with 'Play: <name>'",
            ));
            return ParseOutcome {
                ast: None,
                diagnostics,
            };
        }
    }

    // Optional Description (directly after Play).
    if let Some(line) = lines.get(i) {
        if line.depth == 0 && line.content.starts_with("Description:") {
            match header_text(line, "Description", path) {
                Ok((text, _)) => {
                    if strings::free_text_ok(&text) {
                        description = Some(text);
                    } else {
                        diagnostics.push(diag_at(
                            Code::DslMissingHeader,
                            "Description must be 1..=200 characters",
                            line.span(path),
                        ));
                    }
                }
                Err(diag) => diagnostics.push(diag),
            }
            i += 1;
        }
    }

    // Optional App (directly after Play or Description).
    if let Some(line) = lines.get(i) {
        if line.depth == 0 && line.content.starts_with("App:") {
            match header_text(line, "App", path) {
                Ok((text, span)) => {
                    if strings::free_text_ok(&text) && !text.contains(char::is_whitespace) {
                        app = Some(Named { name: text, span });
                    } else {
                        diagnostics.push(diag_at(
                            Code::DslMissingHeader,
                            "App must name an app definition (no spaces), e.g. 'App: shop'",
                            line.span(path),
                        ));
                    }
                }
                Err(diag) => diagnostics.push(diag),
            }
            i += 1;
        }
    }

    // Var declarations.
    while let Some(line) = lines.get(i) {
        if line.depth == 0 && line.content.starts_with("Var:") {
            match parse_var_line(line, path) {
                Ok(var) => {
                    // Last declaration wins (documented in DECISIONS.md).
                    if let Some(existing) = var_decls.iter_mut().find(|v| v.name == var.name) {
                        *existing = var;
                    } else {
                        var_decls.push(var);
                    }
                }
                Err(diag) => diagnostics.push(diag),
            }
            i += 1;
        } else {
            break;
        }
    }

    // Acts.
    while i < lines.len() {
        let Some(line) = lines.get(i) else { break };
        if line.depth != 0 {
            diagnostics.push(diag_at(
                Code::DslMissingHeader,
                "this line is indented but no Stage or Scene block is open",
                line.span(path),
            ));
            i += 1;
            continue;
        }
        if line.content.starts_with("Var:") {
            diagnostics.push(diag_at(
                Code::DslMissingHeader,
                "Var: declarations belong before the first Act",
                line.span(path),
            ));
            i += 1;
            continue;
        }
        if line.content.starts_with("Description:") {
            diagnostics.push(diag_at(
                Code::DslMissingHeader,
                "Description: must directly follow the Play: line",
                line.span(path),
            ));
            i += 1;
            continue;
        }
        if !line.content.starts_with("Act:") {
            let keyword = line.content.split(':').next().unwrap_or("").trim();
            if PLAY_KEYWORDS.contains(&keyword) {
                diagnostics.push(diag_at(
                    Code::DslMissingHeader,
                    format!("'{keyword}:' is not valid here"),
                    line.span(path),
                ));
            } else {
                let hint = suggest(keyword, PLAY_KEYWORDS)
                    .map(|h| format!("; {h}"))
                    .unwrap_or_default();
                diagnostics.push(diag_at(
                    Code::DslUnknownKeyword,
                    format!("unknown keyword '{keyword}'{hint}"),
                    line.span(path),
                ));
            }
            i += 1;
            continue;
        }

        // Act header.
        let mut act_name: Option<(String, Span)> = None;
        match header_text(line, "Act", path) {
            Ok((text, span)) => {
                if strings::free_text_ok(&text) {
                    act_name = Some((text, span));
                } else {
                    diagnostics.push(diag_at(
                        Code::DslMissingHeader,
                        "Act: name must be 1..=200 characters",
                        span,
                    ));
                }
            }
            Err(diag) => diagnostics.push(diag),
        }
        i += 1;

        // Optional Tags / Platforms directly after the Act line.
        let mut tags: Vec<Named> = Vec::new();
        let mut platforms: Vec<Named> = Vec::new();
        if let Some(next) = lines.get(i) {
            if next.depth == 0 && next.content.starts_with("Tags:") {
                match parse_tags_line(next, path) {
                    Ok(list) => tags = list,
                    Err(diag) => diagnostics.push(diag),
                }
                i += 1;
            }
        }
        if let Some(next) = lines.get(i) {
            if next.depth == 0 && next.content.starts_with("Platforms:") {
                match parse_platforms_line(next, path) {
                    Ok(list) => platforms = list,
                    Err(diag) => diagnostics.push(diag),
                }
                i += 1;
            }
        }

        // Stages.
        let mut stages: Vec<StageDecl> = Vec::new();
        while let Some(next) = lines.get(i) {
            if next.depth == 0 && next.content.starts_with("Stage:") {
                let (stage, consumed) = parse_stage(&lines, i, path, &mut diagnostics);
                i += consumed;
                if let Some(stage) = stage {
                    if let Some(previous) = stages.iter().find(|s| s.name == stage.name) {
                        diagnostics.push(
                            Diagnostic::error(
                                Code::DslDuplicateActStage,
                                format!("stage '{}' is declared twice in this Act", stage.name),
                            )
                            .with_span(stage.name_span.clone())
                            .with_help(format!(
                                "first declared at line {}",
                                previous.name_span.line
                            )),
                        );
                    } else {
                        stages.push(stage);
                    }
                }
            } else {
                break;
            }
        }

        if stages.is_empty() {
            diagnostics.push(diag_at(
                Code::DslMissingHeader,
                "an Act needs at least one Stage",
                line.span(path),
            ));
        }

        if let Some((act_name, act_span)) = act_name {
            if let Some(previous) = acts.iter().find(|a| a.name == act_name) {
                diagnostics.push(
                    Diagnostic::error(
                        Code::DslDuplicateActStage,
                        format!("act '{act_name}' is declared twice in this play"),
                    )
                    .with_span(act_span)
                    .with_help(format!(
                        "first declared at line {}",
                        previous.name_span.line
                    )),
                );
            } else {
                acts.push(ActDecl {
                    name: act_name,
                    name_span: act_span,
                    tags,
                    platforms,
                    stages,
                });
            }
        }
    }

    if acts.is_empty() {
        diagnostics.push(Diagnostic::error(
            Code::DslMissingHeader,
            "a play needs at least one Act",
        ));
    }

    let ast = match (name, name_span) {
        (Some(name), Some(name_span)) => Some(PlayFile {
            path: PathBuf::from(path),
            name,
            name_span,
            description,
            app,
            var_decls,
            acts,
        }),
        _ => None,
    };
    ParseOutcome { ast, diagnostics }
}

fn header_text(
    line: &crate::scanner::LogicalLine,
    keyword: &str,
    path: &Path,
) -> Result<(String, Span), Diagnostic> {
    let prefix = format!("{keyword}:");
    let rest = line.content.get(prefix.len()..).unwrap_or("");
    let trimmed = rest.trim();
    let leading = rest.len() - rest.trim_start().len();
    let span = Span::new(
        path,
        line.line,
        line.col + prefix.chars().count() + leading,
        trimmed.chars().count(),
    );
    if trimmed.is_empty() {
        return Err(diag_at(
            Code::DslMissingHeader,
            format!("{keyword}: needs a value"),
            span,
        ));
    }
    Ok((trimmed.to_string(), span))
}

fn parse_var_line(line: &crate::scanner::LogicalLine, path: &Path) -> Result<VarDecl, Diagnostic> {
    let mut cursor = Cursor::new(&line.content, path, line.line, line.col);
    let _ = cursor.try_lit("Var");
    cursor.expect_lit(":").map_err(|_| {
        diag_at(
            Code::DslUnknownKeyword,
            "malformed Var: line",
            line.span(path),
        )
    })?;
    let name = cursor.ident().map_err(|reason| {
        diag_at(
            Code::DslUnknownKeyword,
            format!("Var: {reason}"),
            line.span(path),
        )
    })?;
    cursor.expect_lit("=").map_err(|_| {
        diag_at(
            Code::DslUnknownKeyword,
            "Var: NAME = 'value' needs the '=' sign",
            line.span(path),
        )
    })?;
    let value = cursor.quoted().map_err(|reason| {
        diag_at(
            Code::DslUnterminatedString,
            format!("Var value must be a quoted string ({reason})"),
            cursor.span(cursor.col(), 1),
        )
    })?;
    if !cursor.at_end() {
        return Err(diag_at(
            Code::DslUnknownKeyword,
            "unexpected trailing input after the Var value",
            cursor.span(cursor.col(), 1),
        ));
    }
    Ok(VarDecl {
        name: name.name,
        name_span: name.span,
        value,
    })
}

fn parse_tags_line(
    line: &crate::scanner::LogicalLine,
    path: &Path,
) -> Result<Vec<Named>, Diagnostic> {
    let body_col = line.col + "Tags:".len();
    let body = line.content.get("Tags:".len()..).unwrap_or("");
    let mut out = Vec::new();
    for part in body.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !trimmed
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        {
            return Err(diag_at(
                Code::DslUnknownKeyword,
                format!("invalid tag '{trimmed}'"),
                Span::new(path, line.line, body_col + 1, trimmed.chars().count()),
            ));
        }
        // Re-validate through the winnow grammar (single source of truth).
        let mut tag_rest = trimmed;
        if grammar::tag.parse_next(&mut tag_rest).is_err() || !tag_rest.is_empty() {
            return Err(diag_at(
                Code::DslUnknownKeyword,
                format!("invalid tag '{trimmed}'"),
                Span::new(path, line.line, body_col + 1, trimmed.chars().count()),
            ));
        }
        let col = body
            .find(trimmed)
            .map(|byte| body.get(..byte).map(|p| p.chars().count()).unwrap_or(0))
            .unwrap_or(0);
        out.push(Named {
            name: trimmed.to_string(),
            span: Span::new(path, line.line, body_col + col + 1, trimmed.chars().count()),
        });
    }
    if out.is_empty() {
        return Err(diag_at(
            Code::DslUnknownKeyword,
            "Tags: needs at least one tag",
            line.span(path),
        ));
    }
    Ok(out)
}

fn parse_platforms_line(
    line: &crate::scanner::LogicalLine,
    path: &Path,
) -> Result<Vec<Named>, Diagnostic> {
    let body_col = line.col + "Platforms:".len();
    let body = line.content.get("Platforms:".len()..).unwrap_or("");
    let mut out = Vec::new();
    for part in body.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        match trimmed.parse::<Platform>() {
            Ok(platform) => {
                let col = body
                    .find(trimmed)
                    .map(|byte| body.get(..byte).map(|p| p.chars().count()).unwrap_or(0))
                    .unwrap_or(0);
                out.push(Named {
                    name: platform.as_str().to_string(),
                    span: Span::new(path, line.line, body_col + col + 1, trimmed.chars().count()),
                });
            }
            Err(()) => {
                let names: Vec<&str> = Platform::ALL.iter().map(|p| p.as_str()).collect();
                let hint = suggest(trimmed, &names)
                    .map(|h| format!("; {h}"))
                    .unwrap_or_default();
                return Err(diag_at(
                    Code::DslUnknownKeyword,
                    format!("unknown platform '{trimmed}' in Platforms:{hint}"),
                    line.span(path),
                ));
            }
        }
    }
    if out.is_empty() {
        return Err(diag_at(
            Code::DslUnknownKeyword,
            "Platforms: needs at least one platform",
            line.span(path),
        ));
    }
    Ok(out)
}

/// Parse one Stage starting at `lines[start]` (a `Stage:` line).
/// Returns the stage and the number of lines consumed.
fn parse_stage(
    lines: &[crate::scanner::LogicalLine],
    start: usize,
    path: &Path,
    diagnostics: &mut Diagnostics,
) -> (Option<StageDecl>, usize) {
    let mut consumed = 0usize;
    let Some(header) = lines.get(start) else {
        return (None, 1);
    };
    consumed += 1;
    let (name, name_span) = match header_text(header, "Stage", path) {
        Ok((text, span)) if strings::free_text_ok(&text) => (text, span),
        Ok((_, span)) => {
            diagnostics.push(diag_at(
                Code::DslMissingHeader,
                "Stage: name must be 1..=200 characters",
                span.clone(),
            ));
            ("".to_string(), span)
        }
        Err(diag) => {
            diagnostics.push(diag);
            ("".to_string(), header.span(path))
        }
    };

    let mut timeout: Option<DslDuration> = None;
    let mut timeout_span: Option<Span> = None;
    let mut scenes: Vec<SceneBlock> = Vec::new();

    loop {
        let Some(line) = lines.get(start + consumed) else {
            break;
        };
        if line.depth == 0 {
            break;
        }
        if line.depth != 1 {
            diagnostics.push(diag_at(
                Code::DslMissingHeader,
                "unexpected indentation inside a Stage",
                line.span(path),
            ));
            consumed += 1;
            continue;
        }

        if line.content.starts_with("Timeout:") {
            if timeout.is_some() {
                diagnostics.push(diag_at(
                    Code::DslDuplicateActStage,
                    "Timeout: is given twice in this Stage",
                    line.span(path),
                ));
                consumed += 1;
                continue;
            }
            if !scenes.is_empty() {
                diagnostics.push(diag_at(
                    Code::DslMissingHeader,
                    "Timeout: must come before the first Scene block of the Stage",
                    line.span(path),
                ));
                consumed += 1;
                continue;
            }
            let body = line.content.get("Timeout:".len()..).unwrap_or("").trim();
            let span = Span::new(
                path,
                line.line,
                line.col + "Timeout:".len() + 1,
                body.chars().count(),
            );
            match crate::duration::parse(body, Some(span.clone())) {
                Ok(duration) => {
                    timeout = Some(duration);
                    timeout_span = Some(span);
                }
                Err(diag) => diagnostics.push(diag),
            }
            consumed += 1;
            continue;
        }

        if line.content.starts_with("Scene:") {
            let (block, used) = parse_scene_block(lines, start + consumed, path, diagnostics);
            consumed += used;
            if let Some(block) = block {
                scenes.push(block);
            }
            continue;
        }

        let keyword = line.content.split(':').next().unwrap_or("").trim();
        if PLAY_KEYWORDS.contains(&keyword) {
            diagnostics.push(diag_at(
                Code::DslMissingHeader,
                format!("'{keyword}:' is not valid inside a Stage"),
                line.span(path),
            ));
        } else {
            let hint = suggest(keyword, PLAY_KEYWORDS)
                .map(|h| format!("; {h}"))
                .unwrap_or_default();
            diagnostics.push(diag_at(
                Code::DslUnknownKeyword,
                format!("unknown keyword '{keyword}'{hint}"),
                line.span(path),
            ));
        }
        consumed += 1;
    }

    if scenes.is_empty() {
        diagnostics.push(diag_at(
            Code::DslMissingHeader,
            "a Stage needs at least one Scene block",
            name_span.clone(),
        ));
    }

    (
        Some(StageDecl {
            name,
            name_span,
            timeout,
            timeout_span,
            scenes,
        }),
        consumed,
    )
}

/// Parse one `Scene: X` block starting at `lines[start]`.
fn parse_scene_block(
    lines: &[crate::scanner::LogicalLine],
    start: usize,
    path: &Path,
    diagnostics: &mut Diagnostics,
) -> (Option<SceneBlock>, usize) {
    let mut consumed = 0usize;
    let Some(header) = lines.get(start) else {
        return (None, 1);
    };
    consumed += 1;
    let mut cursor = Cursor::new(
        header.content.get("Scene:".len()..).unwrap_or(""),
        path,
        header.line,
        header.col + "Scene:".len(),
    );
    let scene_name = match cursor.ident() {
        Ok(ident) => {
            if !cursor.at_end() {
                diagnostics.push(diag_at(
                    Code::DslUnknownKeyword,
                    "unexpected input after the scene name",
                    cursor.span(cursor.col(), 1),
                ));
            }
            ident
        }
        Err(reason) => {
            diagnostics.push(diag_at(
                Code::DslMissingHeader,
                format!("Scene: needs a scene name ({reason})"),
                header.span(path),
            ));
            Ident {
                name: String::new(),
                span: header.span(path),
            }
        }
    };

    let mut steps: Vec<Step> = Vec::new();
    let mut saw_step_line = false;
    loop {
        let Some(line) = lines.get(start + consumed) else {
            break;
        };
        if line.depth <= 1 {
            break;
        }
        if line.depth != 2 {
            diagnostics.push(diag_at(
                Code::DslMissingHeader,
                "unexpected indentation inside a Scene block",
                line.span(path),
            ));
            consumed += 1;
            continue;
        }
        consumed += 1;
        if line.content.starts_with("Action:") {
            saw_step_line = true;
            match parse_action_line(line, path) {
                Ok(step) => steps.push(step),
                Err(diag) => diagnostics.push(diag),
            }
        } else if line.content.starts_with("Check:") {
            saw_step_line = true;
            match parse_check_line(line, path) {
                Ok(step) => steps.push(step),
                Err(diag) => diagnostics.push(diag),
            }
        } else {
            let keyword = line.content.split(':').next().unwrap_or("").trim();
            if PLAY_KEYWORDS.contains(&keyword) {
                diagnostics.push(diag_at(
                    Code::DslMissingHeader,
                    format!("'{keyword}:' cannot appear inside a Scene block"),
                    line.span(path),
                ));
            } else {
                let hint = suggest(keyword, &["Action", "Check"])
                    .map(|h| format!("; {h}"))
                    .unwrap_or_default();
                diagnostics.push(diag_at(
                    Code::DslUnknownKeyword,
                    format!("expected 'Action:' or 'Check:', found '{keyword}'{hint}"),
                    line.span(path),
                ));
            }
        }
    }

    if steps.is_empty() && !saw_step_line {
        diagnostics.push(diag_at(
            Code::DslMissingHeader,
            "a Scene block needs at least one step",
            header.span(path),
        ));
    }

    (
        Some(SceneBlock {
            scene_name: scene_name.name,
            scene_span: scene_name.span,
            steps,
        }),
        consumed,
    )
}

fn parse_action_line(line: &crate::scanner::LogicalLine, path: &Path) -> Result<Step, Diagnostic> {
    let body_col = line.col + "Action:".len();
    let body_start = line.content.get("Action:".len()..).unwrap_or("");
    let leading = body_start.len() - body_start.trim_start().len();
    let body = body_start.trim();
    let mut cursor = Cursor::new(body, path, line.line, body_col + leading);
    let kind = parse_action_body(&mut cursor)?;

    let mut timeout = None;
    if cursor.try_lit("timeout") {
        let duration = cursor.duration().map_err(|reason| {
            diag_at(
                Code::DslInvalidDuration,
                reason,
                cursor.span(cursor.col(), 1),
            )
        })?;
        timeout = Some(duration);
    }
    if !cursor.at_end() {
        return Err(diag_at(
            Code::DslUnknownAction,
            format!("unexpected trailing input '{}'", cursor.rest.trim()),
            cursor.span(cursor.col(), cursor.rest.trim().chars().count().max(1)),
        ));
    }

    // Timeout is not allowed on key/back/wait/screenshot/app (IMP-DSL-022).
    if timeout.is_some() && kind.timeout_forbidden() {
        return Err(diag_at(
            Code::DslTimeoutNotAllowed,
            format!("`{}` does not take a timeout", kind.name()),
            Span::new(path, line.line, body_col + 1, body.chars().count()),
        ));
    }

    Ok(Step {
        source_text: line.content.clone(),
        span: line.span(path),
        timeout,
        kind: StepKind::Action(Action {
            verb: kind,
            span: Span::new(path, line.line, body_col + 1, body.chars().count()),
        }),
    })
}

impl ActionVerb {
    fn timeout_forbidden(&self) -> bool {
        matches!(
            self,
            ActionVerb::Key { .. }
                | ActionVerb::Back
                | ActionVerb::Wait { .. }
                | ActionVerb::Screenshot { .. }
                | ActionVerb::App { .. }
        )
    }
}

fn parse_action_body(cursor: &mut Cursor<'_>) -> Result<ActionVerb, Diagnostic> {
    // Shorthand: `'text' => Piece`
    if cursor.rest.trim_start().starts_with(['\'', '"']) {
        let text = cursor.quoted().map_err(|reason| {
            diag_at(
                Code::DslUnterminatedString,
                format!("invalid string ({reason})"),
                cursor.span(cursor.col(), 1),
            )
        })?;
        cursor.expect_lit("=>").map_err(|_| {
            diag_at(
                Code::DslUnknownAction,
                "expected '=>' after the text",
                cursor.span(cursor.col(), 1),
            )
        })?;
        let piece = cursor.ident().map_err(|reason| {
            diag_at(
                Code::DslUnknownAction,
                format!("expected a piece after '=>' ({reason})"),
                cursor.span(cursor.col(), 1),
            )
        })?;
        return Ok(ActionVerb::Type { text, piece });
    }

    let (word, span) = cursor.word().map_err(|_| {
        diag_at(
            Code::DslUnknownAction,
            "expected an action verb",
            cursor.span(cursor.col(), 1),
        )
    })?;

    let piece = |cursor: &mut Cursor<'_>| -> Result<Ident, Diagnostic> {
        cursor.expect_lit("=>").map_err(|_| {
            diag_at(
                Code::DslUnknownAction,
                format!("`{word}` needs '=> Piece'"),
                cursor.span(cursor.col(), 1),
            )
        })?;
        cursor.ident().map_err(|reason| {
            diag_at(
                Code::DslUnknownAction,
                format!("expected a piece after '=>' ({reason})"),
                cursor.span(cursor.col(), 1),
            )
        })
    };
    let text = |cursor: &mut Cursor<'_>| -> Result<ParsedString, Diagnostic> {
        cursor.quoted().map_err(|reason| {
            diag_at(
                Code::DslUnterminatedString,
                format!("`{word}` needs a quoted string ({reason})"),
                cursor.span(cursor.col(), 1),
            )
        })
    };

    match word {
        "type" => {
            let text = text(cursor)?;
            cursor.expect_lit("=>").map_err(|_| {
                diag_at(
                    Code::DslUnknownAction,
                    "expected '=>' after the text",
                    cursor.span(cursor.col(), 1),
                )
            })?;
            let piece = cursor.ident().map_err(|reason| {
                diag_at(
                    Code::DslUnknownAction,
                    format!("expected a piece after '=>' ({reason})"),
                    cursor.span(cursor.col(), 1),
                )
            })?;
            Ok(ActionVerb::Type { text, piece })
        }
        "append" => {
            let text = text(cursor)?;
            cursor.expect_lit("=>").map_err(|_| {
                diag_at(
                    Code::DslUnknownAction,
                    "expected '=>' after the text",
                    cursor.span(cursor.col(), 1),
                )
            })?;
            let piece = cursor.ident().map_err(|reason| {
                diag_at(
                    Code::DslUnknownAction,
                    format!("expected a piece after '=>' ({reason})"),
                    cursor.span(cursor.col(), 1),
                )
            })?;
            Ok(ActionVerb::Append { text, piece })
        }
        "clear" => Ok(ActionVerb::Clear {
            piece: piece(cursor)?,
        }),
        "click" => Ok(ActionVerb::Click {
            piece: piece(cursor)?,
        }),
        "longpress" => {
            let duration = if cursor
                .rest
                .trim_start()
                .starts_with(|c: char| c.is_ascii_digit())
            {
                cursor.duration().map_err(|reason| {
                    diag_at(
                        Code::DslInvalidDuration,
                        format!("invalid longpress duration ({reason})"),
                        cursor.span(cursor.col(), 1),
                    )
                })?
            } else {
                // Default longpress duration is 1000ms (spec 5.5).
                DslDuration::ONE_SECOND
            };
            let piece = piece(cursor)?;
            Ok(ActionVerb::LongPress { duration, piece })
        }
        "focus" => Ok(ActionVerb::Focus {
            piece: piece(cursor)?,
        }),
        "select" => {
            let option = text(cursor)?;
            cursor.expect_lit("=>").map_err(|_| {
                diag_at(
                    Code::DslUnknownAction,
                    "expected '=>' after the option text",
                    cursor.span(cursor.col(), 1),
                )
            })?;
            let piece = cursor.ident().map_err(|reason| {
                diag_at(
                    Code::DslUnknownAction,
                    format!("expected a piece after '=>' ({reason})"),
                    cursor.span(cursor.col(), 1),
                )
            })?;
            Ok(ActionVerb::Select { option, piece })
        }
        "scroll" => {
            let direction = parse_direction(cursor, word)?;
            let count = if cursor
                .rest
                .trim_start()
                .starts_with(|c: char| c.is_ascii_digit())
            {
                cursor.uint().map_err(|_| {
                    diag_at(
                        Code::DslUnknownAction,
                        "scroll count must be an integer",
                        cursor.span(cursor.col(), 1),
                    )
                })?
            } else {
                1
            };
            let piece = if cursor.try_lit("=>") {
                Some(cursor.ident().map_err(|reason| {
                    diag_at(
                        Code::DslUnknownAction,
                        format!("expected a piece after '=>' ({reason})"),
                        cursor.span(cursor.col(), 1),
                    )
                })?)
            } else {
                None
            };
            Ok(ActionVerb::Scroll {
                direction,
                count,
                piece,
            })
        }
        "scrollto" => Ok(ActionVerb::ScrollTo {
            piece: piece(cursor)?,
        }),
        "swipe" => {
            let direction = parse_direction(cursor, word)?;
            let piece = if cursor.try_lit("=>") {
                Some(cursor.ident().map_err(|reason| {
                    diag_at(
                        Code::DslUnknownAction,
                        format!("expected a piece after '=>' ({reason})"),
                        cursor.span(cursor.col(), 1),
                    )
                })?)
            } else {
                None
            };
            Ok(ActionVerb::Swipe { direction, piece })
        }
        "key" => {
            let (key, word, span) = cursor.keyname().map_err(|word| {
                diag_at(
                    Code::DslUnknownKeyName,
                    "key needs a key name",
                    cursor.span(cursor.col(), word.chars().count().max(1)),
                )
            })?;
            if word.is_empty() {
                return Err(diag_at(
                    Code::DslUnknownKeyName,
                    "key needs a key name",
                    span,
                ));
            }
            let repeat = if cursor
                .rest
                .trim_start()
                .starts_with(|c: char| c.is_ascii_digit())
            {
                cursor.uint().map_err(|_| {
                    diag_at(
                        Code::DslUnknownAction,
                        "key repeat count must be an integer",
                        cursor.span(cursor.col(), 1),
                    )
                })?
            } else {
                1
            };
            Ok(ActionVerb::Key {
                key: KeyName { key, span },
                repeat,
            })
        }
        "back" => Ok(ActionVerb::Back),
        "wait" => {
            let duration = cursor.duration().map_err(|reason| {
                diag_at(
                    Code::DslInvalidDuration,
                    format!("wait needs a duration ({reason})"),
                    cursor.span(cursor.col(), 1),
                )
            })?;
            Ok(ActionVerb::Wait { duration })
        }
        "screenshot" => {
            let name = text(cursor)?;
            Ok(ActionVerb::Screenshot { name })
        }
        "app" => {
            let op = parse_app_op(cursor).map_err(|reason| {
                diag_at(Code::DslUnknownAction, reason, cursor.span(cursor.col(), 1))
            })?;
            Ok(ActionVerb::App { op })
        }
        other => {
            let verbs = [
                "type",
                "append",
                "clear",
                "click",
                "longpress",
                "focus",
                "select",
                "scroll",
                "scrollto",
                "swipe",
                "key",
                "back",
                "wait",
                "screenshot",
                "app",
            ];
            let hint = suggest(other, &verbs)
                .map(|h| format!("; {h}"))
                .unwrap_or_default();
            Err(diag_at(
                Code::DslUnknownAction,
                format!("unknown action '{other}'{hint}"),
                span,
            ))
        }
    }
}

fn parse_direction(cursor: &mut Cursor<'_>, verb: &str) -> Result<Direction, Diagnostic> {
    match cursor.direction() {
        Ok(direction) => Ok(direction),
        Err(other) => {
            let hint = suggest(&other, &["up", "down", "left", "right"])
                .map(|h| format!("; {h}"))
                .unwrap_or_default();
            let message = if other.is_empty() {
                format!("`{verb}` needs a direction (up, down, left, right)")
            } else {
                format!("unknown direction '{other}'{hint}")
            };
            Err(diag_at(
                Code::DslUnknownAction,
                message,
                cursor.span(cursor.col(), other.chars().count().max(1)),
            ))
        }
    }
}

fn parse_check_line(line: &crate::scanner::LogicalLine, path: &Path) -> Result<Step, Diagnostic> {
    let body_col = line.col + "Check:".len();
    let body_start = line.content.get("Check:".len()..).unwrap_or("");
    let leading = body_start.len() - body_start.trim_start().len();
    let body = body_start.trim();
    let mut cursor = Cursor::new(body, path, line.line, body_col + leading);

    let piece = cursor.ident().map_err(|reason| {
        diag_at(
            Code::DslUnknownAction,
            format!("Check: needs a piece name ({reason})"),
            cursor.span(cursor.col(), 1),
        )
    })?;

    let condition = if cursor.try_lit("is") {
        let (word, span) = cursor.word().map_err(|_| {
            diag_at(
                Code::DslUnknownAction,
                "`is` needs a condition (visible, hidden, enabled, disabled, focused)",
                cursor.span(cursor.col(), 1),
            )
        })?;
        match word {
            "visible" => CheckCondition::IsVisible,
            "hidden" => CheckCondition::IsHidden,
            "enabled" => CheckCondition::IsEnabled,
            "disabled" => CheckCondition::IsDisabled,
            "focused" => CheckCondition::IsFocused,
            other => {
                let hint = suggest(
                    other,
                    &["visible", "hidden", "enabled", "disabled", "focused"],
                )
                .map(|h| format!("; {h}"))
                .unwrap_or_default();
                return Err(diag_at(
                    Code::DslUnknownKeyword,
                    format!("unknown condition 'is {other}'{hint}"),
                    span,
                ));
            }
        }
    } else if cursor.try_lit("textContains") {
        let text = cursor.quoted().map_err(|reason| {
            diag_at(
                Code::DslUnterminatedString,
                format!("textContains needs a quoted string ({reason})"),
                cursor.span(cursor.col(), 1),
            )
        })?;
        CheckCondition::TextContains { text }
    } else if cursor.try_lit("text") {
        let text = cursor.quoted().map_err(|reason| {
            diag_at(
                Code::DslUnterminatedString,
                format!("text needs a quoted string ({reason})"),
                cursor.span(cursor.col(), 1),
            )
        })?;
        CheckCondition::TextEquals { text }
    } else if cursor.try_lit("count") {
        let count = cursor.uint().map_err(|_| {
            diag_at(
                Code::DslUnknownAction,
                "count needs a non-negative integer",
                cursor.span(cursor.col(), 1),
            )
        })?;
        CheckCondition::Count { count }
    } else {
        CheckCondition::IsVisible
    };

    let mut timeout = None;
    if cursor.try_lit("timeout") {
        let duration = cursor.duration().map_err(|reason| {
            diag_at(
                Code::DslInvalidDuration,
                reason,
                cursor.span(cursor.col(), 1),
            )
        })?;
        timeout = Some(duration);
    }
    if !cursor.at_end() {
        return Err(diag_at(
            Code::DslUnknownAction,
            format!("unexpected trailing input '{}'", cursor.rest.trim()),
            cursor.span(cursor.col(), cursor.rest.trim().chars().count().max(1)),
        ));
    }

    Ok(Step {
        source_text: line.content.clone(),
        span: line.span(path),
        timeout,
        kind: StepKind::Check(Check {
            piece,
            condition,
            span: Span::new(path, line.line, body_col + 1, body.chars().count()),
        }),
    })
}

/// Parse the `AppOp` from an action body (helper so `app` above stays flat).
fn parse_app_op(cursor: &mut Cursor<'_>) -> Result<AppOp, String> {
    cursor.skip_ws();
    let mut input = cursor.rest;
    match grammar::app_op.parse_next(&mut input) {
        Ok(op) => {
            cursor.rest = input;
            Ok(op)
        }
        Err(_) => Err("app needs one of: launch, terminate, restart, background".to_string()),
    }
}
