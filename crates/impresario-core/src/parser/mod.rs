//! File parsers: hand-written structure (indentation, headers, recovery)
//! over the winnow line grammars in [`grammar`].
//!
//! Rules implemented here (spec 5.2, 5.4, 5.5): recovering (collect as many
//! diagnostics as possible), never panics, one `Step` per Action/Check line.

mod grammar;
pub(crate) mod table;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use crate::ast::{
    ActDecl, Action, ActionVerb, AppOp, Argument, Check, CheckCondition, Direction, DrmTerm,
    ExampleColumn, ExampleRow, ExampleTable, Ident, KeyName, Named, ParamDecl, PerformDecl,
    PieceDecl, PieceSelector, PlayFile, PlayerAssertion, PlayerCheck, SceneBlock, SceneColumn,
    SceneFile, ShowingCheck, ShowingOption, StageDecl, StageItem, StatusTerm, Step, StepKind,
    TheatreFile, VarDecl,
};
use crate::diag::{suggest, Code, Diagnostic, Diagnostics};
use crate::duration::DslDuration;
use crate::keys::RemoteKey;
use crate::platforms::{Platform, ScenePlatforms};
use crate::selectors;
use crate::span::Span;
use crate::strings::{self, ParsedString};

/// Result of parsing one file.
#[derive(Debug)]
pub struct ParseOutcome<T> {
    pub ast: Option<T>,
    pub diagnostics: Diagnostics,
}

#[derive(Debug, Clone, Copy)]
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

/// Parse a `.scene` file. The tabular form is the canonical one; the legacy
/// `Platform:`/`Pieces:` form is still accepted for `migrate` (I-110).
#[must_use]
pub fn parse_scene(source: &str, path: &Path) -> ParseOutcome<SceneFile> {
    let scanned = crate::scanner::scan(source, path);
    let mut rest = scanned.lines.iter().skip(1);
    let second = rest.next();
    let tabular = match second {
        Some(line)
            if line.depth == 0
                && (line.content.starts_with("Platforms:")
                    || line.content.starts_with("Platform:")) =>
        {
            rest.next()
                .is_some_and(|next| next.depth == 0 && next.content.starts_with('|'))
        }
        Some(line) => line.depth == 0 && line.content.starts_with('|'),
        None => false,
    };
    if tabular {
        return parse_scene_table(source, path);
    }
    parse_scene_legacy(source, path)
}

fn parse_scene_legacy(source: &str, path: &Path) -> ParseOutcome<SceneFile> {
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
            columns: Vec::new(),
            pieces,
        }),
        _ => None,
    };

    ParseOutcome { ast, diagnostics }
}

fn parse_scene_table(source: &str, path: &Path) -> ParseOutcome<SceneFile> {
    let scanned = crate::scanner::scan(source, path);
    let mut diagnostics = Diagnostics::new();
    diagnostics.extend(scanned.diagnostics);
    let lines = scanned.lines;

    let Some(header) = lines.first() else {
        diagnostics.push(Diagnostic::error(
            Code::DslMissingHeader,
            "a scene file must start with 'Scene: <Name>'",
        ));
        return ParseOutcome {
            ast: None,
            diagnostics,
        };
    };
    if header.depth != 0 || !header.content.starts_with("Scene:") {
        diagnostics.push(diag_at(
            Code::DslMissingHeader,
            "a scene file must start with 'Scene: <Name>'",
            header.span(path),
        ));
        return ParseOutcome {
            ast: None,
            diagnostics,
        };
    }
    let (scene_name, name_span) = match header_text(header, "Scene", path) {
        Ok((text, span)) if strings::free_text_ok(&text) && !text.contains(char::is_whitespace) => {
            (Some(text), Some(span))
        }
        Ok((_, span)) => {
            diagnostics.push(diag_at(
                Code::DslMissingHeader,
                "Scene: name must be a single identifier",
                span,
            ));
            (None, None)
        }
        Err(diag) => {
            diagnostics.push(diag);
            (None, None)
        }
    };

    let mut i = 1usize;
    let mut platforms: Option<ScenePlatforms> = None;
    let mut platforms_span: Option<Span> = None;
    if let Some(line) = lines.get(i) {
        if line.depth == 0
            && (line.content.starts_with("Platforms:") || line.content.starts_with("Platform:"))
        {
            let keyword = if line.content.starts_with("Platforms:") {
                "Platforms:"
            } else {
                "Platform:"
            };
            let body_col = line.col + keyword.len();
            let body = line.content.get(keyword.len()..).unwrap_or("").trim();
            let span = Span::new(
                path,
                line.line,
                body_col + leading_ws(line, keyword),
                body.chars().count(),
            );
            match parse_platform_list(body) {
                Ok(list) => {
                    platforms = Some(list);
                    platforms_span = Some(span);
                }
                Err(reason) => diagnostics.push(diag_at(Code::DslUnknownKeyword, reason, span)),
            }
            i += 1;
        }
    }
    let platforms = platforms.unwrap_or(ScenePlatforms::CrossPlatform);
    let platforms_span =
        platforms_span.unwrap_or_else(|| name_span.clone().unwrap_or_else(|| header.span(path)));

    let Some(table_header) = lines.get(i) else {
        diagnostics.push(diag_at(
            Code::DslMissingHeader,
            "a tabular scene needs a '| Piece | ... |' header",
            header.span(path),
        ));
        return ParseOutcome {
            ast: None,
            diagnostics,
        };
    };
    if table_header.depth != 0 || !table_header.content.starts_with('|') {
        diagnostics.push(diag_at(
            Code::DslMissingHeader,
            "expected the scene table after the header",
            table_header.span(path),
        ));
        return ParseOutcome {
            ast: None,
            diagnostics,
        };
    }
    let header_cells = match table::split_row(table_header, path) {
        Ok(cells) => cells,
        Err(diag) => {
            diagnostics.push(diag);
            return ParseOutcome {
                ast: None,
                diagnostics,
            };
        }
    };
    i += 1;

    let mut columns: Vec<SceneColumn> = Vec::new();
    let mut column_positions: Vec<usize> = Vec::new();
    for (index, cell) in header_cells.iter().enumerate() {
        let name = cell.text.trim();
        if index == 0 {
            if !name.eq_ignore_ascii_case("Piece") {
                diagnostics.push(diag_at(
                    Code::DslMissingHeader,
                    "the first table column must be 'Piece'",
                    cell.span.clone(),
                ));
            }
            continue;
        }
        if name.is_empty() {
            diagnostics.push(diag_at(
                Code::DslUnknownKeyword,
                "a table column needs a platform or 'index'",
                cell.span.clone(),
            ));
            continue;
        }
        if name.eq_ignore_ascii_case("index") {
            if columns.iter().any(|c| c.platform.is_none()) {
                diagnostics.push(diag_at(
                    Code::DslInvalidSelector,
                    "the index column is declared twice",
                    cell.span.clone(),
                ));
                continue;
            }
            columns.push(SceneColumn {
                platform: None,
                name: "index".to_string(),
                span: cell.span.clone(),
            });
            column_positions.push(index);
            continue;
        }
        let platform = match name.parse::<Platform>() {
            Ok(platform) => platform,
            Err(()) => {
                let names: Vec<&str> = Platform::ALL.iter().map(|p| p.as_str()).collect();
                let hint = suggest(name, &names)
                    .map(|h| format!("; {h}"))
                    .unwrap_or_default();
                diagnostics.push(diag_at(
                    Code::DslInvalidSelector,
                    format!("unknown table column '{name}'{hint}"),
                    cell.span.clone(),
                ));
                continue;
            }
        };
        if columns.iter().any(|c| c.platform == Some(platform)) {
            diagnostics.push(diag_at(
                Code::DslInvalidSelector,
                format!("column '{platform}' is declared twice"),
                cell.span.clone(),
            ));
            continue;
        }
        columns.push(SceneColumn {
            platform: Some(platform),
            name: platform.as_str().to_string(),
            span: cell.span.clone(),
        });
        column_positions.push(index);
    }

    let mut pieces: Vec<PieceDecl> = Vec::new();
    while let Some(line) = lines.get(i) {
        if line.depth != 0 {
            diagnostics.push(diag_at(
                Code::DslUnknownKeyword,
                "table rows are not indented",
                line.span(path),
            ));
            i += 1;
            continue;
        }
        if !line.content.starts_with('|') {
            break;
        }
        let cells = match table::split_row(line, path) {
            Ok(cells) => cells,
            Err(diag) => {
                diagnostics.push(diag);
                i += 1;
                continue;
            }
        };
        i += 1;
        if cells.iter().all(table::Cell::is_separator) {
            continue;
        }
        if cells.len() != header_cells.len() {
            diagnostics.push(diag_at(
                Code::DslInvalidSelector,
                format!(
                    "piece row has {} cell(s), but the header has {}",
                    cells.len(),
                    header_cells.len()
                ),
                line.span(path),
            ));
            continue;
        }
        let Some(piece_cell) = cells.first() else {
            continue;
        };
        let piece_name = piece_cell.text.trim();
        if piece_name.is_empty() {
            diagnostics.push(diag_at(
                Code::DslDuplicatePiece,
                "a piece row needs a name",
                piece_cell.span.clone(),
            ));
            continue;
        }
        let mut name_input = piece_name;
        let piece_ident = match grammar::ident.parse_next(&mut name_input) {
            Ok(name) if name_input.is_empty() => name,
            _ => {
                diagnostics.push(diag_at(
                    Code::DslDuplicatePiece,
                    format!("'{piece_name}' is not a valid piece name"),
                    piece_cell.span.clone(),
                ));
                continue;
            }
        };

        let mut index = 0u32;
        let mut selectors: Vec<PieceSelector> = Vec::new();
        for (column, position) in columns.iter().zip(column_positions.iter()) {
            let Some(cell) = cells.get(*position) else {
                continue;
            };
            if let Some(platform) = column.platform {
                if cell.absent {
                    continue;
                }
                selectors.push(PieceSelector {
                    key: platform,
                    raw: cell.text.clone(),
                    span: cell.span.clone(),
                });
            } else if !cell.absent {
                index = match cell.text.trim().parse::<u32>() {
                    Ok(value) => value,
                    Err(_) => {
                        diagnostics.push(diag_at(
                            Code::DslInvalidSelector,
                            "index must be a non-negative integer",
                            cell.span.clone(),
                        ));
                        0
                    }
                };
            }
        }
        for selector in &selectors {
            if let Err(diag) = selectors::parse(
                selector.key,
                &selector.raw,
                index,
                Some(selector.span.clone()),
            ) {
                diagnostics.push(diag);
            }
        }
        if selectors.is_empty() {
            diagnostics.push(
                Diagnostic::warning(
                    Code::DslMissingSelectorForPlatform,
                    format!("piece '{piece_ident}' has no selector on any platform"),
                )
                .with_span(piece_cell.span.clone()),
            );
        }
        if let Some(previous) = pieces.iter().find(|p| p.name == piece_ident) {
            diagnostics.push(
                Diagnostic::error(
                    Code::DslDuplicatePiece,
                    format!("piece '{piece_ident}' is declared twice"),
                )
                .with_span(piece_cell.span.clone())
                .with_help(format!(
                    "first declared at line {}",
                    previous.name_span.line
                )),
            );
            continue;
        }
        pieces.push(PieceDecl {
            name: piece_ident,
            name_span: piece_cell.span.clone(),
            span: line.span(path),
            selectors,
            index,
        });
    }

    if pieces.is_empty() && diagnostics.errors().count() == 0 {
        diagnostics.push(Diagnostic::error(
            Code::DslMissingHeader,
            "a scene needs at least one piece",
        ));
    }

    let ast = match (scene_name, name_span) {
        (Some(name), Some(name_span)) => Some(SceneFile {
            path: PathBuf::from(path),
            name,
            name_span,
            platforms,
            platforms_span,
            columns,
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
        let mut probe = cursor;
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

/// Parse a `.play` file. A play that uses `Perform:`/`Examples:` is parsed
/// with the theatrical grammar; the legacy inline-`Stage:` form is still
/// accepted until `migrate` rewrites it (I-110).
#[must_use]
pub fn parse_play(source: &str, path: &Path) -> ParseOutcome<PlayFile> {
    let scanned = crate::scanner::scan(source, path);
    let theatrical = scanned.lines.iter().any(|line| {
        (line.depth == 1
            && (line.content.starts_with("Perform:") || line.content.starts_with("Examples:")))
            || (line.depth == 0 && line.content.starts_with("Act:") && line.content.contains('('))
    });
    if theatrical {
        return parse_play_theatrical(source, path);
    }
    parse_play_legacy(source, path)
}

fn parse_play_legacy(source: &str, path: &Path) -> ParseOutcome<PlayFile> {
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
                        let span = var.name_span.clone();
                        diagnostics.push(
                            Diagnostic::warning(
                                Code::DslLintShadowedVar,
                                format!("variable '{}' is declared more than once", var.name),
                            )
                            .with_span(span)
                            .with_help("the last declaration wins; rename or remove one"),
                        );
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
                    params: Vec::new(),
                    tags,
                    platforms,
                    examples: None,
                    performs: Vec::new(),
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

/// Parse a `.theatre` file (TASK-2 section 8.3).
#[must_use]
pub fn parse_theatre(source: &str, path: &Path) -> ParseOutcome<TheatreFile> {
    let scanned = crate::scanner::scan(source, path);
    let mut diagnostics = Diagnostics::new();
    diagnostics.extend(scanned.diagnostics);
    let lines = scanned.lines;

    let mut name: Option<String> = None;
    let mut name_span: Option<Span> = None;
    match lines.first() {
        Some(line) if line.depth == 0 && line.content.starts_with("Theatre:") => {
            match header_text(line, "Theatre", path) {
                Ok((text, span)) if strings::free_text_ok(&text) => {
                    name = Some(text);
                    name_span = Some(span);
                }
                Ok((_, span)) => diagnostics.push(diag_at(
                    Code::DslMissingHeader,
                    "Theatre: name must be 1..=200 characters",
                    span,
                )),
                Err(diag) => diagnostics.push(diag),
            }
        }
        Some(line) => diagnostics.push(diag_at(
            Code::DslMissingHeader,
            "a theatre file must start with 'Theatre: <name>'",
            line.span(path),
        )),
        None => {
            diagnostics.push(Diagnostic::error(
                Code::DslMissingHeader,
                "a theatre file must start with 'Theatre: <name>'",
            ));
            return ParseOutcome {
                ast: None,
                diagnostics,
            };
        }
    }

    let mut stages: Vec<StageDecl> = Vec::new();
    let mut i = 1usize;
    while i < lines.len() {
        let Some(line) = lines.get(i) else { break };
        if line.depth != 0 {
            diagnostics.push(diag_at(
                Code::DslMissingHeader,
                "this line is indented but no Stage is open",
                line.span(path),
            ));
            i += 1;
            continue;
        }
        if !line.content.starts_with("Stage:") {
            let keyword = line.content.split(':').next().unwrap_or("").trim();
            diagnostics.push(diag_at(
                Code::DslUnknownKeyword,
                format!(
                    "unknown keyword '{keyword}' in a theatre{}",
                    suggest(keyword, &["Stage"])
                        .map(|h| format!("; {h}"))
                        .unwrap_or_default()
                ),
                line.span(path),
            ));
            i += 1;
            continue;
        }
        let (stage, used) = parse_theatre_stage(&lines, i, path, &mut diagnostics);
        i += used;
        if let Some(stage) = stage {
            if let Some(previous) = stages.iter().find(|s| s.name == stage.name) {
                diagnostics.push(
                    Diagnostic::error(
                        Code::DslDuplicateStage,
                        format!("stage '{}' is declared twice in this theatre", stage.name),
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
    }

    if stages.is_empty() && diagnostics.errors().count() == 0 {
        diagnostics.push(Diagnostic::error(
            Code::DslMissingHeader,
            "a theatre needs at least one Stage",
        ));
    }

    let ast = match (name, name_span) {
        (Some(name), Some(name_span)) => Some(TheatreFile {
            path: PathBuf::from(path),
            name,
            name_span,
            stages,
        }),
        _ => None,
    };
    ParseOutcome { ast, diagnostics }
}

fn parse_theatre_stage(
    lines: &[crate::scanner::LogicalLine],
    start: usize,
    path: &Path,
    diagnostics: &mut Diagnostics,
) -> (Option<StageDecl>, usize) {
    let Some(header) = lines.get(start) else {
        return (None, 1);
    };
    let mut consumed = 1usize;
    let (stage_name, params) = match header_text(header, "Stage", path) {
        Ok((text, span)) => match parse_parameterised_header(&text, &span, path, diagnostics) {
            Some(pair) => pair,
            None => ("".to_string(), Vec::new()),
        },
        Err(diag) => {
            diagnostics.push(diag);
            ("".to_string(), Vec::new())
        }
    };
    let name_span = Span::new(
        path,
        header.line,
        header.col + "Stage:".len() + leading_ws(header, "Stage:"),
        stage_name.chars().count(),
    );

    let mut description: Option<String> = None;
    let mut platforms: Vec<Named> = Vec::new();
    let mut timeout: Option<DslDuration> = None;
    let mut timeout_span: Option<Span> = None;
    let mut items: Vec<StageItem> = Vec::new();
    let mut scenes: Vec<SceneBlock> = Vec::new();
    let mut saw_item = false;

    while let Some(line) = lines.get(start + consumed) {
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
        if line.content.starts_with("Description:") {
            if description.is_some() || saw_item {
                diagnostics.push(diag_at(
                    Code::DslMissingHeader,
                    "Description: must come once, before the stage body",
                    line.span(path),
                ));
                consumed += 1;
                continue;
            }
            match header_text(line, "Description", path) {
                Ok((text, _)) if strings::free_text_ok(&text) => description = Some(text),
                Ok((_, span)) => diagnostics.push(diag_at(
                    Code::DslMissingHeader,
                    "Description must be 1..=200 characters",
                    span,
                )),
                Err(diag) => diagnostics.push(diag),
            }
            consumed += 1;
            continue;
        }
        if line.content.starts_with("Platforms:") {
            if !platforms.is_empty() || saw_item {
                diagnostics.push(diag_at(
                    Code::DslMissingHeader,
                    "Platforms: must come once, before the stage body",
                    line.span(path),
                ));
                consumed += 1;
                continue;
            }
            match parse_platforms_line(line, path) {
                Ok(list) => platforms = list,
                Err(diag) => diagnostics.push(diag),
            }
            consumed += 1;
            continue;
        }
        if line.content.starts_with("Timeout:") {
            if timeout.is_some() || saw_item {
                diagnostics.push(diag_at(
                    Code::DslDuplicateActStage,
                    "Timeout: is given twice or after the stage body",
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
            saw_item = true;
            let (block, used) = parse_scene_block(lines, start + consumed, path, diagnostics);
            consumed += used;
            if let Some(block) = block {
                items.push(StageItem::Scene(block.clone()));
                scenes.push(block);
            }
            continue;
        }
        if line.content.starts_with("Perform:") {
            saw_item = true;
            match parse_perform_line(line, path) {
                Ok(perform) => items.push(StageItem::Perform(perform)),
                Err(diag) => diagnostics.push(diag),
            }
            consumed += 1;
            continue;
        }
        let keyword = line.content.split(':').next().unwrap_or("").trim();
        diagnostics.push(diag_at(
            Code::DslUnknownKeyword,
            format!(
                "expected Scene:, Perform:, Platforms:, Timeout: or Description:, found '{keyword}'{}",
                suggest(keyword, &["Scene", "Perform", "Platforms", "Timeout", "Description"])
                    .map(|h| format!("; {h}"))
                    .unwrap_or_default()
            ),
            line.span(path),
        ));
        consumed += 1;
    }

    if !saw_item && diagnostics.errors().count() == 0 {
        diagnostics.push(diag_at(
            Code::DslMissingHeader,
            "a Stage needs at least one Scene or Perform",
            name_span.clone(),
        ));
    }

    (
        Some(StageDecl {
            name: stage_name,
            name_span,
            params,
            description,
            platforms,
            timeout,
            timeout_span,
            items,
            scenes,
        }),
        consumed,
    )
}

fn parse_play_theatrical(source: &str, path: &Path) -> ParseOutcome<PlayFile> {
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

    match lines.first() {
        Some(line) if line.depth == 0 && line.content.starts_with("Play:") => {
            match header_text(line, "Play", path) {
                Ok((text, span)) if strings::free_text_ok(&text) => {
                    name = Some(text);
                    name_span = Some(span);
                }
                Ok((_, span)) => diagnostics.push(diag_at(
                    Code::DslMissingHeader,
                    "Play: name must be 1..=200 characters",
                    span,
                )),
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

    if let Some(line) = lines.get(i) {
        if line.depth == 0 && line.content.starts_with("Description:") {
            if let Ok((text, _)) = header_text(line, "Description", path) {
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
            i += 1;
        }
    }
    if let Some(line) = lines.get(i) {
        if line.depth == 0 && line.content.starts_with("App:") {
            if let Ok((text, span)) = header_text(line, "App", path) {
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
            i += 1;
        }
    }
    while let Some(line) = lines.get(i) {
        if line.depth == 0 && line.content.starts_with("Var:") {
            match parse_var_line(line, path) {
                Ok(var) => {
                    if let Some(existing) = var_decls.iter_mut().find(|v| v.name == var.name) {
                        let span = var.name_span.clone();
                        diagnostics.push(
                            Diagnostic::warning(
                                Code::DslLintShadowedVar,
                                format!("variable '{}' is declared more than once", var.name),
                            )
                            .with_span(span)
                            .with_help("the last declaration wins; rename or remove one"),
                        );
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

    while i < lines.len() {
        let Some(line) = lines.get(i) else { break };
        if line.depth != 0 {
            diagnostics.push(diag_at(
                Code::DslMissingHeader,
                "this line is indented but no Act is open",
                line.span(path),
            ));
            i += 1;
            continue;
        }
        if !line.content.starts_with("Act:") {
            let keyword = line.content.split(':').next().unwrap_or("").trim();
            let hint = suggest(keyword, PLAY_KEYWORDS)
                .map(|h| format!("; {h}"))
                .unwrap_or_default();
            diagnostics.push(diag_at(
                Code::DslUnknownKeyword,
                format!("unknown keyword '{keyword}' outside an Act{hint}"),
                line.span(path),
            ));
            i += 1;
            continue;
        }

        let (act_name, act_span, params) = match header_text(line, "Act", path) {
            Ok((text, span)) => {
                match parse_parameterised_header(&text, &span, path, &mut diagnostics) {
                    Some((name, params)) => (Some((name, span.clone())), Some(span), params),
                    None => (None, Some(span), Vec::new()),
                }
            }
            Err(diag) => {
                diagnostics.push(diag);
                (None, None, Vec::new())
            }
        };
        i += 1;

        let mut tags: Vec<Named> = Vec::new();
        let mut platforms: Vec<Named> = Vec::new();
        let mut examples: Option<ExampleTable> = None;

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
        if let Some(next) = lines.get(i) {
            if next.depth == 1 && next.content.starts_with("Examples:") {
                let (table, used) = parse_examples(&lines, i, path, &mut diagnostics);
                examples = table;
                i += used;
            }
        }

        let mut performs: Vec<PerformDecl> = Vec::new();
        while let Some(next) = lines.get(i) {
            if next.depth == 0 {
                break;
            }
            if next.depth != 1 {
                diagnostics.push(diag_at(
                    Code::DslMissingHeader,
                    "Perform: lines are indented exactly one level under the Act",
                    next.span(path),
                ));
                i += 1;
                continue;
            }
            if next.content.starts_with("Perform:") {
                match parse_perform_line(next, path) {
                    Ok(perform) => performs.push(perform),
                    Err(diag) => diagnostics.push(diag),
                }
                i += 1;
            } else if next.content.starts_with("Stage:") {
                diagnostics.push(
                    diag_at(
                        Code::DslMissingHeader,
                        "inline Stage: blocks are not allowed in an Act",
                        next.span(path),
                    )
                    .with_help("move the stage to a .theatre file and use `Perform: <Stage>`"),
                );
                i += 1;
                // Skip the whole legacy block so the rest of the Act still parses.
                while let Some(skip) = lines.get(i) {
                    if skip.depth == 0 {
                        break;
                    }
                    i += 1;
                }
            } else {
                let keyword = next.content.split(':').next().unwrap_or("").trim();
                diagnostics.push(diag_at(
                    Code::DslUnknownKeyword,
                    format!(
                        "expected 'Perform:', found '{keyword}'{}",
                        suggest(keyword, &["Perform", "Examples"])
                            .map(|h| format!("; {h}"))
                            .unwrap_or_default()
                    ),
                    next.span(path),
                ));
                i += 1;
            }
        }

        if performs.is_empty() {
            diagnostics.push(diag_at(
                Code::DslMissingHeader,
                "an Act needs at least one Perform: line",
                act_span.clone().unwrap_or_else(|| line.span(path)),
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
                    params,
                    tags,
                    platforms,
                    examples,
                    performs,
                    stages: Vec::new(),
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

/// `Name` or `Name(param, other = 'default')` used by Act/Stage headers.
fn parse_parameterised_header(
    text: &str,
    span: &Span,
    path: &Path,
    diagnostics: &mut Diagnostics,
) -> Option<(String, Vec<ParamDecl>)> {
    let (name, params) = match text.find('(') {
        Some(open) if text.trim_end().ends_with(')') => {
            let name = text.get(..open).unwrap_or("").trim();
            let inner_end = text.trim_end().len().saturating_sub(1);
            let inner = text.get(open + 1..inner_end).unwrap_or("");
            let params = parse_params(inner, path, span.line, span.col + open + 1, diagnostics);
            (name, params)
        }
        _ => (text.trim(), Vec::new()),
    };
    if !strings::free_text_ok(name) {
        diagnostics.push(diag_at(
            Code::DslMissingHeader,
            "the name must be 1..=200 characters",
            span.clone(),
        ));
        return None;
    }
    Some((name.to_string(), params))
}

fn parse_params(
    body: &str,
    path: &Path,
    line: usize,
    base_col: usize,
    diagnostics: &mut Diagnostics,
) -> Vec<ParamDecl> {
    let mut cursor = Cursor::new(body, path, line, base_col);
    let mut out: Vec<ParamDecl> = Vec::new();
    loop {
        cursor.skip_ws();
        if cursor.at_end() {
            break;
        }
        let name = match cursor.ident() {
            Ok(ident) => ident,
            Err(reason) => {
                diagnostics.push(diag_at(
                    Code::DslUnknownParameter,
                    format!("expected a parameter name ({reason})"),
                    cursor.span(cursor.col(), 1),
                ));
                break;
            }
        };
        if out.iter().any(|p: &ParamDecl| p.name == name.name) {
            diagnostics.push(diag_at(
                Code::DslDuplicateParameter,
                format!("parameter '{}' is declared twice", name.name),
                name.span.clone(),
            ));
        }
        cursor.skip_ws();
        let default = if cursor.try_lit("=") {
            match cursor.quoted() {
                Ok(value) => Some(value),
                Err(reason) => {
                    diagnostics.push(diag_at(
                        Code::DslUnterminatedString,
                        format!(
                            "parameter '{}' needs a quoted default ({reason})",
                            name.name
                        ),
                        cursor.span(cursor.col(), 1),
                    ));
                    None
                }
            }
        } else {
            None
        };
        out.push(ParamDecl {
            name: name.name,
            name_span: name.span,
            default,
        });
        cursor.skip_ws();
        if cursor.try_lit(",") {
            continue;
        }
        break;
    }
    if !cursor.at_end() {
        diagnostics.push(diag_at(
            Code::DslUnknownParameter,
            format!(
                "unexpected input '{}' in parameter list",
                cursor.rest.trim()
            ),
            cursor.span(cursor.col(), cursor.rest.trim().chars().count().max(1)),
        ));
    }
    out
}

fn parse_perform_line(
    line: &crate::scanner::LogicalLine,
    path: &Path,
) -> Result<PerformDecl, Diagnostic> {
    let body_start = line.content.get("Perform:".len()..).unwrap_or("");
    let leading = body_start.len() - body_start.trim_start().len();
    let body = body_start.trim();
    let body_col = line.col + "Perform:".len() + leading;
    let (stage_name, args) = match body.find('(') {
        Some(open) if body.trim_end().ends_with(')') => {
            let stage_name = body.get(..open).unwrap_or("").trim();
            let inner_end = body.trim_end().len().saturating_sub(1);
            let inner = body.get(open + 1..inner_end).unwrap_or("");
            (stage_name, Some((inner, body_col + open + 1)))
        }
        _ => (body.trim(), None),
    };
    if !strings::free_text_ok(stage_name) {
        return Err(diag_at(
            Code::DslUnknownStage,
            "Perform: needs a stage name",
            line.span(path),
        ));
    }
    let mut arguments: Vec<Argument> = Vec::new();
    if let Some((inner, inner_col)) = args {
        let mut cursor = Cursor::new(inner, path, line.line, inner_col);
        loop {
            cursor.skip_ws();
            if cursor.at_end() {
                break;
            }
            let name = cursor.ident().map_err(|reason| {
                diag_at(
                    Code::DslInvalidStageArguments,
                    format!("expected an argument name ({reason})"),
                    cursor.span(cursor.col(), 1),
                )
            })?;
            cursor.expect_lit("=").map_err(|_| {
                diag_at(
                    Code::DslInvalidStageArguments,
                    format!("argument '{}' needs '= value'", name.name),
                    cursor.span(cursor.col(), 1),
                )
            })?;
            let value = cursor.quoted().map_err(|reason| {
                diag_at(
                    Code::DslInvalidStageArguments,
                    format!("argument '{}' needs a quoted value ({reason})", name.name),
                    cursor.span(cursor.col(), 1),
                )
            })?;
            if arguments.iter().any(|a| a.name == name.name) {
                return Err(diag_at(
                    Code::DslInvalidStageArguments,
                    format!("argument '{}' is given twice", name.name),
                    name.span,
                ));
            }
            arguments.push(Argument {
                name: name.name,
                name_span: name.span,
                value,
            });
            cursor.skip_ws();
            if cursor.try_lit(",") {
                continue;
            }
            break;
        }
        if !cursor.at_end() {
            return Err(diag_at(
                Code::DslInvalidStageArguments,
                format!("unexpected input '{}' in argument list", cursor.rest.trim()),
                cursor.span(cursor.col(), cursor.rest.trim().chars().count().max(1)),
            ));
        }
    }
    Ok(PerformDecl {
        stage_name: stage_name.to_string(),
        stage_span: Span::new(path, line.line, body_col, stage_name.chars().count()),
        args: arguments,
        span: line.span(path),
    })
}

fn parse_examples(
    lines: &[crate::scanner::LogicalLine],
    start: usize,
    path: &Path,
    diagnostics: &mut Diagnostics,
) -> (Option<ExampleTable>, usize) {
    let mut used = 1usize; // Examples:
    let header_line = match lines.get(start + used) {
        Some(line) if line.depth == 2 && line.content.starts_with('|') => line,
        Some(line) => {
            diagnostics.push(diag_at(
                Code::DslMissingHeader,
                "Examples: needs an indented table header",
                line.span(path),
            ));
            return (None, used);
        }
        None => {
            diagnostics.push(Diagnostic::error(
                Code::DslMissingHeader,
                "Examples: needs a table",
            ));
            return (None, used);
        }
    };
    let header_cells = match table::split_row(header_line, path) {
        Ok(cells) => cells,
        Err(diag) => {
            diagnostics.push(diag);
            return (None, used);
        }
    };
    used += 1;
    let mut columns: Vec<ExampleColumn> = Vec::new();
    for cell in &header_cells {
        let name = cell.text.trim().to_string();
        if name.is_empty() {
            diagnostics.push(diag_at(
                Code::DslMissingHeader,
                "an Examples column needs a name",
                cell.span.clone(),
            ));
            continue;
        }
        if !name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            diagnostics.push(diag_at(
                Code::DslUnknownExampleColumn,
                format!("example column '{name}' is not an identifier"),
                cell.span.clone(),
            ));
            continue;
        }
        if columns.iter().any(|c| c.name == name) {
            diagnostics.push(diag_at(
                Code::DslDuplicateExampleColumn,
                format!("example column '{name}' is declared twice"),
                cell.span.clone(),
            ));
            continue;
        }
        columns.push(ExampleColumn {
            name,
            span: cell.span.clone(),
        });
    }
    if columns.is_empty() {
        return (None, used);
    }

    let mut rows: Vec<ExampleRow> = Vec::new();
    while let Some(line) = lines.get(start + used) {
        if line.depth != 2 {
            break;
        }
        if !line.content.starts_with('|') {
            break;
        }
        let cells = match table::split_row(line, path) {
            Ok(cells) => cells,
            Err(diag) => {
                diagnostics.push(diag);
                used += 1;
                continue;
            }
        };
        used += 1;
        if cells.iter().all(table::Cell::is_separator) {
            continue;
        }
        if cells.len() != columns.len() {
            diagnostics.push(diag_at(
                Code::DslExampleWidth,
                format!(
                    "example row has {} cell(s), but the header has {}",
                    cells.len(),
                    columns.len()
                ),
                line.span(path),
            ));
            continue;
        }
        rows.push(ExampleRow {
            values: cells
                .into_iter()
                .map(|cell| if cell.absent { None } else { Some(cell.text) })
                .collect(),
            span: line.span(path),
        });
    }
    if rows.is_empty() {
        diagnostics.push(diag_at(
            Code::DslMissingHeader,
            "Examples: needs at least one data row",
            header_line.span(path),
        ));
    }
    let span = table::table_span(&header_cells, &header_cells, path);
    (
        Some(ExampleTable {
            columns,
            rows,
            span,
        }),
        used,
    )
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

    while let Some(line) = lines.get(start + consumed) {
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
            params: Vec::new(),
            description: None,
            platforms: Vec::new(),
            timeout,
            timeout_span,
            items: Vec::new(),
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
    while let Some(line) = lines.get(start + consumed) {
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

    cursor.skip_ws();
    let (piece, condition) = if starts_keyword(cursor.rest, "Showing") {
        cursor.try_lit("Showing");
        parse_showing_check(&mut cursor, false)?
    } else if starts_keyword(cursor.rest, "not") {
        let mut after_not = cursor.rest.trim_start();
        after_not = after_not.get("not".len()..).unwrap_or("");
        after_not = after_not.trim_start();
        if starts_keyword(after_not, "Showing") {
            cursor.try_lit("not");
            cursor.try_lit("Showing");
            parse_showing_check(&mut cursor, true)?
        } else {
            (
                cursor.ident().map_err(|reason| {
                    diag_at(
                        Code::DslUnknownAction,
                        format!("Check: needs a piece name ({reason})"),
                        cursor.span(cursor.col(), 1),
                    )
                })?,
                parse_legacy_condition(&mut cursor)?,
            )
        }
    } else if starts_keyword(cursor.rest, "Player") {
        cursor.try_lit("Player");
        parse_player_check(&mut cursor)?
    } else {
        let piece = cursor.ident().map_err(|reason| {
            diag_at(
                Code::DslUnknownAction,
                format!("Check: needs a piece name ({reason})"),
                cursor.span(cursor.col(), 1),
            )
        })?;
        (piece, parse_legacy_condition(&mut cursor)?)
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

fn starts_keyword(input: &str, keyword: &str) -> bool {
    match input.strip_prefix(keyword) {
        Some(rest) => rest.is_empty() || rest.starts_with([' ', '\t']),
        None => false,
    }
}

fn parse_showing_check(
    cursor: &mut Cursor<'_>,
    negated: bool,
) -> Result<(Ident, CheckCondition), Diagnostic> {
    cursor.expect_lit("=>").map_err(|_| {
        diag_at(
            Code::DslUnknownAction,
            "`Showing` needs `=>` and a piece name",
            cursor.span(cursor.col(), 1),
        )
    })?;
    let piece = cursor.ident().map_err(|reason| {
        diag_at(
            Code::DslUnknownAction,
            format!("`Showing =>` needs a piece name ({reason})"),
            cursor.span(cursor.col(), 1),
        )
    })?;
    let mut options = Vec::new();
    if cursor.try_lit("with") {
        loop {
            let option = if cursor.try_lit("coverage") {
                cursor.expect_lit(">=").map_err(|_| {
                    diag_at(
                        Code::DslUnknownAction,
                        "`coverage` needs `>=N%`",
                        cursor.span(cursor.col(), 1),
                    )
                })?;
                let percent = cursor.uint().map_err(|_| {
                    diag_at(
                        Code::DslUnknownAction,
                        "`coverage` needs an integer percentage",
                        cursor.span(cursor.col(), 1),
                    )
                })?;
                cursor.expect_lit("%").map_err(|_| {
                    diag_at(
                        Code::DslUnknownAction,
                        "`coverage` percentage needs a trailing `%`",
                        cursor.span(cursor.col(), 1),
                    )
                })?;
                let percent = u8::try_from(percent).map_err(|_| {
                    diag_at(
                        Code::DslUnknownAction,
                        "`coverage` must be between 0% and 100%",
                        cursor.span(cursor.col(), 1),
                    )
                })?;
                if percent > 100 {
                    return Err(diag_at(
                        Code::DslUnknownAction,
                        "`coverage` must be between 0% and 100%",
                        cursor.span(cursor.col(), 1),
                    ));
                }
                ShowingOption::Coverage { percent }
            } else if cursor.try_lit("decoded") {
                ShowingOption::Decoded
            } else if cursor.try_lit("source") {
                cursor.expect_lit("~").map_err(|_| {
                    diag_at(
                        Code::DslUnknownAction,
                        "`source` needs `~\"substring\"`",
                        cursor.span(cursor.col(), 1),
                    )
                })?;
                let text = cursor.quoted().map_err(|reason| {
                    diag_at(
                        Code::DslUnterminatedString,
                        format!("`source~` needs a quoted substring ({reason})"),
                        cursor.span(cursor.col(), 1),
                    )
                })?;
                ShowingOption::Source { contains: text.raw }
            } else {
                return Err(diag_at(
                    Code::DslUnknownAction,
                    format!(
                        "unknown Showing option '{}' (coverage>=N%, decoded, source~\"…\")",
                        cursor.peek_word()
                    ),
                    cursor.span(cursor.col(), cursor.peek_word().chars().count().max(1)),
                ));
            };
            options.push(option);
            if !cursor.try_lit(",") {
                break;
            }
        }
    }
    Ok((
        piece,
        CheckCondition::Showing(ShowingCheck { negated, options }),
    ))
}

fn parse_player_check(cursor: &mut Cursor<'_>) -> Result<(Ident, CheckCondition), Diagnostic> {
    cursor.expect_lit("=>").map_err(|_| {
        diag_at(
            Code::DslUnknownAction,
            "`Player` needs `=>` and a piece name",
            cursor.span(cursor.col(), 1),
        )
    })?;
    let piece = cursor.ident().map_err(|reason| {
        diag_at(
            Code::DslUnknownAction,
            format!("`Player =>` needs a piece name ({reason})"),
            cursor.span(cursor.col(), 1),
        )
    })?;
    let mut status = None;
    let mut drm = None;
    let mut assertions = Vec::new();
    loop {
        cursor.skip_ws();
        if cursor.at_end() || starts_keyword(cursor.rest, "timeout") {
            break;
        }
        if cursor.try_lit("with") {
            assertions = parse_player_assertions(cursor)?;
            break;
        }
        if cursor.try_lit("DRM-enabled") {
            let (provider, provider_negated) = parse_optional_provider(cursor)?;
            drm = Some(DrmTerm {
                enabled: true,
                provider,
                provider_negated,
            });
            continue;
        }
        if cursor.try_lit("DRM-disabled") {
            let (provider, provider_negated) = parse_optional_provider(cursor)?;
            drm = Some(DrmTerm {
                enabled: false,
                provider,
                provider_negated,
            });
            continue;
        }
        let negated = cursor.try_lit("not");
        let (word, span) = cursor.word().map_err(|_| {
            diag_at(
                Code::DslUnknownAction,
                "`Player` needs a status, `DRM-…`, or `with`",
                cursor.span(cursor.col(), 1),
            )
        })?;
        if let Some(parsed) = crate::probe::PlayerStatus::parse(word) {
            if status.is_some() {
                return Err(diag_at(
                    Code::DslUnknownAction,
                    "only one player status may be asserted",
                    span,
                ));
            }
            status = Some(StatusTerm {
                status: parsed,
                negated,
            });
        } else {
            let hint = suggest(word, &player_status_names())
                .map(|hint| format!("; {hint}"))
                .unwrap_or_default();
            return Err(diag_at(
                Code::DslUnknownAction,
                format!("unknown player status '{word}'{hint}"),
                span,
            ));
        }
    }
    if status.is_none() && drm.is_none() && assertions.is_empty() {
        return Err(diag_at(
            Code::DslUnknownAction,
            "`Player` needs a status, `DRM-…`, or `with`",
            cursor.span(cursor.col(), 1),
        ));
    }
    Ok((
        piece,
        CheckCondition::Player(PlayerCheck {
            status,
            drm,
            assertions,
        }),
    ))
}

fn player_status_names() -> Vec<&'static str> {
    crate::probe::PlayerStatus::ALL
        .iter()
        .map(|status| status.as_str())
        .collect()
}

fn parse_optional_provider(
    cursor: &mut Cursor<'_>,
) -> Result<(Option<crate::probe::DrmProvider>, bool), Diagnostic> {
    cursor.skip_ws();
    let mut probe = *cursor;
    let negated = probe.try_lit("not");
    let provider = if let Ok((word, _)) = probe.word() {
        crate::probe::DrmProvider::parse(word)
    } else {
        None
    };
    if let Some(provider) = provider {
        *cursor = probe;
        Ok((Some(provider), negated))
    } else {
        Ok((None, false))
    }
}

fn parse_player_assertions(cursor: &mut Cursor<'_>) -> Result<Vec<PlayerAssertion>, Diagnostic> {
    let mut out = Vec::new();
    loop {
        let negated = cursor.try_lit("not");
        let (word, span) = cursor.word().map_err(|_| {
            diag_at(
                Code::DslUnknownAction,
                "`with` needs a property name",
                cursor.span(cursor.col(), 1),
            )
        })?;
        let Some(property) = crate::probe::ProbeProperty::parse(word) else {
            let hint = suggest(word, &probe_property_names())
                .map(|hint| format!("; {hint}"))
                .unwrap_or_default();
            return Err(diag_at(
                Code::DslUnknownAction,
                format!("unknown player property '{word}'{hint}"),
                span,
            ));
        };
        let operator = parse_operator(cursor);
        let value = if operator.is_some() {
            parse_assertion_value(cursor, property)?
        } else {
            String::new()
        };
        out.push(PlayerAssertion {
            property,
            operator,
            value,
            negated,
        });
        if !cursor.try_lit(",") {
            break;
        }
    }
    Ok(out)
}

fn probe_property_names() -> Vec<&'static str> {
    crate::probe::ProbeProperty::ALL
        .iter()
        .map(|property| property.as_str())
        .collect()
}

fn parse_operator(cursor: &mut Cursor<'_>) -> Option<crate::probe::Operator> {
    for (text, operator) in [
        (">=", crate::probe::Operator::Ge),
        ("<=", crate::probe::Operator::Le),
        ("!=", crate::probe::Operator::Ne),
        ("=", crate::probe::Operator::Eq),
        (">", crate::probe::Operator::Gt),
        ("<", crate::probe::Operator::Lt),
        ("~", crate::probe::Operator::Contains),
    ] {
        if cursor.try_lit(text) {
            return Some(operator);
        }
    }
    None
}

fn parse_assertion_value(
    cursor: &mut Cursor<'_>,
    property: crate::probe::ProbeProperty,
) -> Result<String, Diagnostic> {
    cursor.skip_ws();
    if cursor.rest.starts_with('"') {
        let text = cursor.quoted().map_err(|reason| {
            diag_at(
                Code::DslUnterminatedString,
                format!("assertion value needs a quoted string ({reason})"),
                cursor.span(cursor.col(), 1),
            )
        })?;
        return Ok(text.raw);
    }
    let end = cursor
        .rest
        .find([',', ' ', '\t'])
        .unwrap_or(cursor.rest.len());
    let raw = cursor.rest.get(..end).unwrap_or("");
    if raw.is_empty() {
        return Err(diag_at(
            Code::DslUnknownAction,
            "the assertion operator needs a value",
            cursor.span(cursor.col(), 1),
        ));
    }
    cursor.rest = cursor.rest.get(end..).unwrap_or("");
    Ok(canonical_probe_value(raw, property))
}

/// Convert human units to the canonical wire value: durations become
/// seconds, percentages become fractions, resolutions become their vertical
/// pixel count, everything else stays literal.
fn canonical_probe_value(raw: &str, property: crate::probe::ProbeProperty) -> String {
    if property == crate::probe::ProbeProperty::Resolution {
        if let Some(height) = raw.strip_suffix('p').or_else(|| raw.strip_suffix('P')) {
            if height.chars().all(|c| c.is_ascii_digit()) {
                return height.to_string();
            }
        }
        if raw.eq_ignore_ascii_case("4k") {
            return "2160".to_string();
        }
        if raw.eq_ignore_ascii_case("8k") {
            return "4320".to_string();
        }
    }
    if let Some(percent) = raw.strip_suffix('%') {
        if let Ok(value) = percent.parse::<f64>() {
            return format!("{}", value / 100.0);
        }
    }
    if raw.chars().next().is_some_and(|c| c.is_ascii_digit())
        && raw.chars().last().is_some_and(|c| c == 's' || c == 'm')
    {
        if let Ok(duration) = crate::duration::parse(raw, None) {
            return format!("{}", duration.millis() as f64 / 1000.0);
        }
    }
    raw.to_string()
}

fn parse_legacy_condition(cursor: &mut Cursor<'_>) -> Result<CheckCondition, Diagnostic> {
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
    Ok(condition)
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
