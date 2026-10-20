//! Canonical formatter for `.scene`, `.theatre` and `.play` files
//! (TASK-2 section 8.6).
//!
//! The formatter is line-oriented: it keeps comments and intentional single
//! blank lines, canonicalises keyword casing and indentation, aligns tables
//! with display width, and rewrites absent cells as `-`. It never reorders
//! semantics. Formatting is idempotent.

use std::collections::HashMap;
use std::path::Path;

use unicode_width::UnicodeWidthStr;

use crate::diag::{Code, Diagnostic, Diagnostics};
use crate::parser::table::{self, Cell};
use crate::parser::{parse_play, parse_scene, parse_theatre};
use crate::scanner::{self, LogicalLine};

/// Format one source file. Parse errors are returned unchanged and no text is
/// produced; warnings do not block formatting.
pub fn format_source(source: &str, path: &Path) -> Result<String, Diagnostics> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    let diagnostics = match extension {
        "scene" => parse_scene(source, path).diagnostics,
        "theatre" => parse_theatre(source, path).diagnostics,
        "play" => parse_play(source, path).diagnostics,
        _ => {
            let mut diagnostics = Diagnostics::new();
            diagnostics.push(Diagnostic::error(
                Code::CfgInvalidValue,
                format!(
                    "{} is neither a .scene, .theatre nor a .play file",
                    path.display()
                ),
            ));
            return Err(diagnostics);
        }
    };
    if diagnostics.has_errors() {
        return Err(diagnostics);
    }
    Ok(format_valid_source(source, path))
}

/// Format a source string that has already been parsed successfully.
#[must_use]
pub fn format_valid_source(source: &str, path: &Path) -> String {
    let scanned = scanner::scan(source, path);
    let mut logical: HashMap<usize, LogicalLine> = HashMap::new();
    for line in scanned.lines {
        logical.insert(line.line, line);
    }
    let unit = indent_unit(source);

    let raw: Vec<&str> = source.lines().collect();
    let mut lines: Vec<OutLine> = Vec::new();
    let mut index = 0usize;
    while index < raw.len() {
        let line = raw.get(index).copied().unwrap_or_default();
        if line.trim().is_empty() {
            lines.push(OutLine::blank());
            index += 1;
            continue;
        }
        if line.trim_start().starts_with("//") {
            let spaces = leading_spaces(line);
            let depth = spaces / unit;
            lines.push(OutLine::comment(depth, line.trim_end()));
            index += 1;
            continue;
        }
        let Some(logical_line) = logical.get(&(index + 1)) else {
            // Valid files have a logical line for every non-comment line; keep
            // anything unexpected rather than silently dropping it.
            let depth = leading_spaces(line) / unit;
            lines.push(OutLine::other(depth, line.trim_end()));
            index += 1;
            continue;
        };
        if logical_line.content.starts_with('|') {
            let depth = logical_line.depth;
            let mut rows: Vec<String> = Vec::new();
            while index < raw.len() {
                let candidate = raw.get(index).copied().unwrap_or_default();
                if candidate.trim().is_empty() {
                    index += 1;
                    continue;
                }
                if candidate.trim_start().starts_with("//") {
                    // A comment inside a table is kept as its own line; the
                    // table is split around it to preserve order.
                    break;
                }
                match logical.get(&(index + 1)) {
                    Some(next) if next.depth == depth && next.content.starts_with('|') => {
                        rows.push(next.content.clone());
                        index += 1;
                    }
                    _ => break,
                }
            }
            if rows.is_empty() {
                lines.push(OutLine::other(depth, logical_line.content.trim_end()));
                index += 1;
            } else {
                for formatted in format_table(&rows, depth) {
                    lines.push(formatted);
                }
            }
            continue;
        }
        let canonical = canonical_keyword(&logical_line.content);
        lines.push(OutLine::other(logical_line.depth, &canonical));
        index += 1;
    }

    let lines = normalize_blank_lines(lines);
    let mut out = String::new();
    for line in lines {
        match line.kind {
            Kind::Blank => out.push('\n'),
            _ => {
                for _ in 0..line.depth {
                    out.push_str("    ");
                }
                out.push_str(&line.text);
                out.push('\n');
            }
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Blank,
    Comment,
    Other,
    Table,
}

#[derive(Debug, Clone)]
struct OutLine {
    depth: usize,
    text: String,
    kind: Kind,
}

impl OutLine {
    fn blank() -> Self {
        Self {
            depth: 0,
            text: String::new(),
            kind: Kind::Blank,
        }
    }

    fn comment(depth: usize, text: &str) -> Self {
        Self {
            depth,
            text: text.trim().to_string(),
            kind: Kind::Comment,
        }
    }

    fn other(depth: usize, text: &str) -> Self {
        Self {
            depth,
            text: text.trim_end().to_string(),
            kind: Kind::Other,
        }
    }
}

fn leading_spaces(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ').count()
}

fn indent_unit(source: &str) -> usize {
    let mut unit = usize::MAX;
    for line in source.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with("//") {
            continue;
        }
        let spaces = leading_spaces(line);
        if spaces > 0 {
            unit = unit.min(spaces);
        }
    }
    if unit == usize::MAX {
        4
    } else {
        unit.max(1)
    }
}

fn canonical_keyword(content: &str) -> String {
    const KEYWORDS: &[&str] = &[
        "Action",
        "Act",
        "App",
        "Check",
        "Description",
        "Examples",
        "Perform",
        "Pieces",
        "Platform",
        "Platforms",
        "Play",
        "Scene",
        "Stage",
        "Tags",
        "Theatre",
        "Timeout",
        "Var",
    ];
    let Some(colon) = content.find(':') else {
        return content.trim_end().to_string();
    };
    let token = content.get(..colon).unwrap_or("").trim();
    match KEYWORDS
        .iter()
        .find(|keyword| keyword.eq_ignore_ascii_case(token))
    {
        Some(keyword) => {
            let rest = content.get(colon + 1..).unwrap_or("");
            format!("{keyword}:{rest}")
        }
        None => content.trim_end().to_string(),
    }
}

/// Format a contiguous table block. The first non-separator row is the header;
/// a separator is regenerated after it. Absent data cells become `-`.
fn format_table(rows: &[String], depth: usize) -> Vec<OutLine> {
    let parsed: Vec<Vec<Cell>> = rows
        .iter()
        .filter_map(|row| {
            let line = LogicalLine {
                line: 1,
                depth,
                content: row.clone(),
                col: 1,
            };
            table::split_row(&line, Path::new("fmt")).ok()
        })
        .collect();
    if parsed.is_empty() {
        return rows.iter().map(|row| OutLine::other(depth, row)).collect();
    }

    let mut data_rows: Vec<(bool, Vec<String>)> = Vec::new();
    let mut header_done = false;
    for cells in &parsed {
        if cells.iter().all(Cell::is_separator) {
            continue;
        }
        let is_header = !header_done;
        let mut rendered = Vec::with_capacity(cells.len());
        for cell in cells {
            rendered.push(render_cell(cell, is_header));
        }
        header_done = true;
        data_rows.push((is_header, rendered));
    }
    if data_rows.is_empty() {
        return Vec::new();
    }

    let mut widths: Vec<usize> = Vec::new();
    for (_, cells) in &data_rows {
        for (index, cell) in cells.iter().enumerate() {
            let width = cell.width();
            match widths.get_mut(index) {
                Some(existing) => *existing = (*existing).max(width),
                None => widths.push(width),
            }
        }
    }
    if widths.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::new();
    for (row_index, (is_header, cells)) in data_rows.iter().enumerate() {
        let mut text = String::from("|");
        for (index, cell) in cells.iter().enumerate() {
            let width = widths.get(index).copied().unwrap_or(cell.width());
            text.push(' ');
            text.push_str(cell);
            for _ in cell.width()..width {
                text.push(' ');
            }
            text.push_str(" |");
        }
        out.push(OutLine {
            depth,
            text,
            kind: Kind::Table,
        });
        if row_index == 0 && *is_header {
            let mut separator = String::from("|");
            for width in &widths {
                separator.push_str(&"-".repeat(width + 2));
                separator.push('|');
            }
            out.push(OutLine {
                depth,
                text: separator,
                kind: Kind::Table,
            });
        }
    }
    out
}

fn render_cell(cell: &Cell, header: bool) -> String {
    if cell.absent {
        if header {
            return String::new();
        }
        return "-".to_string();
    }
    let text = cell.text.as_str();
    if text.is_empty() {
        if header {
            return String::new();
        }
        return "-".to_string();
    }
    if text.trim() != text || text.contains('|') {
        return quote_cell(text);
    }
    text.to_string()
}

fn quote_cell(text: &str) -> String {
    let quote = if text.contains('\'') { '"' } else { '\'' };
    let mut out = String::new();
    out.push(quote);
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '|' => out.push_str("\\|"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

fn normalize_blank_lines(mut lines: Vec<OutLine>) -> Vec<OutLine> {
    let mut collapsed: Vec<OutLine> = Vec::new();
    for line in lines.drain(..) {
        if line.kind == Kind::Blank {
            if collapsed
                .last()
                .is_some_and(|last| last.kind == Kind::Blank)
            {
                continue;
            }
            if collapsed.is_empty() {
                continue;
            }
            collapsed.push(line);
        } else {
            collapsed.push(line);
        }
    }
    while collapsed
        .last()
        .is_some_and(|line| line.kind == Kind::Blank)
    {
        collapsed.pop();
    }

    // Insert the canonical blank line before top-level declarations and before
    // a scene table. Existing single blank lines elsewhere are preserved.
    let mut out: Vec<OutLine> = Vec::new();
    for line in collapsed {
        let wants_blank =
            line.kind == Kind::Other && line.depth == 0 && is_top_level_header(&line.text);
        let wants_table_blank = line.kind == Kind::Table
            && line.depth == 0
            && !out.last().is_some_and(|last| last.kind == Kind::Table);
        if (wants_blank || wants_table_blank)
            && !out.is_empty()
            && out.last().is_some_and(|last| last.kind != Kind::Blank)
            && !out
                .last()
                .is_some_and(|last| last.text.trim_start().starts_with("//"))
        {
            out.push(OutLine::blank());
        }
        out.push(line);
    }
    out
}

fn is_top_level_header(text: &str) -> bool {
    let trimmed = text.trim_start();
    ["Scene:", "Theatre:", "Play:", "Stage:", "Act:"]
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format(source: &str, name: &str) -> String {
        format_source(source, Path::new(name)).expect("formats")
    }

    #[test]
    fn scene_tables_are_aligned_absent_cells_are_dashes_and_comments_survive() {
        let source = "Scene: LoginScreen\nPlatforms: web, android\n// locators\n| Piece | web | android | index |\n|---|---|---|---|\n| LoginTitle | h1.title | id:login_title | |\n| Promo | .promo | - | 2 |\n";
        let expected = "Scene: LoginScreen\nPlatforms: web, android\n// locators\n| Piece      | web      | android        | index |\n|------------|----------|----------------|-------|\n| LoginTitle | h1.title | id:login_title | -     |\n| Promo      | .promo   | -              | 2     |\n";
        assert_eq!(format(source, "LoginScreen.scene"), expected);
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(64))]

        /// TASK-2 8.6: `fmt(fmt(x)) == fmt(x)` for every valid file kind.
        /// The generator varies blank lines and table padding around a valid
        /// workspace; the formatter must always reach a fixed point.
        #[test]
        fn formatting_reaches_a_fixed_point(
            blank in 0usize..3,
            pad in 0usize..5,
        ) {
            let blanks = "\n".repeat(blank);
            let gap = " ".repeat(pad);
            let scene = format!(
                "Scene: S\nPlatforms: cross-platform\n{blanks}| Piece{gap}| web{gap}| android{gap}|\n|---|---|---|\n| A{gap}| #a | id:a |\n| B{gap}| h1 | - |\n"
            );
            let theatre = format!(
                "Theatre: T\n{blanks}Stage: Fill(user = 'x')\n    Description: Fill it.\n    Scene: S\n        Action: '{{param.user}}' => A\nStage: Other{gap}\n    Scene: S\n        Check: B\n"
            );
            let play = format!(
                "Play: P\nApp: app\n{blanks}Var: U = 'u'\nAct: A\nTags: smoke{gap}\n    Examples:\n        | user |\n        | alice |\n    Perform: Fill(user = '${{example.user}}')\n"
            );
            for (source, name) in
                [(scene, "S.scene"), (theatre, "T.theatre"), (play, "p.play")]
            {
                let once = format_source(&source, Path::new(name)).expect("valid fixture");
                let twice = format_source(&once, Path::new(name)).expect("formatted output parses");
                proptest::prop_assert_eq!(once, twice);
            }
        }
    }

    #[test]
    fn formatting_is_idempotent_for_all_three_file_kinds() {
        let scene = "Scene: S\nPlatforms: cross-platform\n| Piece | web | android |\n|---|---|---|\n| A | #a | id:a |\n";
        let theatre = "Theatre: T\nStage: Fill(user, remember = 'false')\n    Description: Fill it.\n    Timeout: 5s\n    Scene: S\n        Action: '${param.user}' => A\n    Perform: Other\nStage: Other\n    Scene: S\n        Check: A\n";
        let play = "Play: P\nApp: app\nVar: U = 'u'\nAct: A\nTags: smoke\n    Examples:\n        | user |\n        | alice |\n    Perform: Fill(user='${example.user}')\n";
        for (source, name) in [(scene, "S.scene"), (theatre, "T.theatre"), (play, "p.play")] {
            let once = format(source, name);
            let twice = format(&once, name);
            assert_eq!(once, twice, "not idempotent for {name}:\n{once}");
        }
    }

    #[test]
    fn cjk_cells_use_display_width() {
        let source = "Scene: S\n| Piece | web |\n|---|---|\n| A | 中文 |\n";
        let formatted = format(source, "S.scene");
        assert!(formatted.contains("中文"), "{formatted}");
        assert!(formatted.contains("|-------|------|"), "{formatted}");
        assert!(formatted.contains("| web  |"), "{formatted}");
    }

    #[test]
    fn play_and_theatre_canonical_indentation_and_blank_lines() {
        let play = "Play: P\nDescription: D\nApp: app\nAct: A\nTags: smoke\n    Examples:\n        | user | password |\n        | alice | x |\n    Perform: Fill(user='${example.user}')\n";
        let formatted = format(play, "p.play");
        assert!(
            formatted.starts_with("Play: P\nDescription: D\nApp: app\n\nAct: A\n"),
            "{formatted}"
        );
        assert!(formatted.contains("    Examples:\n        | user  | password |\n        |-------|----------|\n        | alice | x        |\n"), "{formatted}");
        assert!(
            formatted.contains("    Perform: Fill(user='${example.user}')\n"),
            "{formatted}"
        );

        let theatre = "Theatre: T\nStage: A\n    Scene: S\n        Check: P\nStage: B\n    Scene: S\n        Check: P\n";
        let formatted = format(theatre, "T.theatre");
        assert!(
            formatted.starts_with("Theatre: T\n\nStage: A\n"),
            "{formatted}"
        );
        assert!(formatted.contains("\n\nStage: B\n"), "{formatted}");
    }

    #[test]
    fn invalid_files_are_not_formatted() {
        let diagnostics =
            format_source("Scene: S\n| broken", Path::new("S.scene")).expect_err("parse error");
        assert!(diagnostics.has_errors());
    }
}
