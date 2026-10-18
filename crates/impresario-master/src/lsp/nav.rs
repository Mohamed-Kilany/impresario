//! Hover, go-to-definition and document symbols (spec section 13).
//!
//! Everything is derived from fresh parses of the requested document (overlay
//! content first) plus the workspace index, so an unsaved buffer works as
//! well as a saved file.

use std::path::Path;

use impresario_core::ast::{ActDecl, PlayFile, SceneFile, StageDecl};
use impresario_core::parser::{parse_play, parse_scene};
use impresario_core::span::Span;
use tower_lsp_server::ls_types::{
    DocumentSymbol, DocumentSymbolResponse, GotoDefinitionResponse, Hover, HoverContents, Location,
    MarkupContent, MarkupKind, Position, Range, SymbolKind,
};

use super::index::{selectors_summary, Index, PieceInfo, SceneInfo};
use super::is_scene_file;
use super::position::{enclosing_scene, indent_of, word_at, Doc, Encoding};
/// The markdown hover of one piece: its per-platform selectors table.
#[must_use]
pub fn piece_markdown(piece: &PieceInfo) -> String {
    let mut out = format!("**{}**\n\n| Platform | Selector |\n|---|---|\n", piece.name);
    for selector in &piece.selectors {
        out.push_str(&format!(
            "| {} | `{}` |\n",
            selector.key.as_str(),
            selector.raw
        ));
    }
    if piece.index > 0 {
        out.push_str(&format!("\n`index = {}`\n", piece.index));
    }
    out
}

/// Hover on a piece (in a step or in its declaration) shows the selectors of
/// every platform. Nothing else has a hover.
#[must_use]
pub fn hover(
    index: &Index,
    text: &str,
    is_scene: bool,
    pos: Position,
    enc: Encoding,
) -> Option<Hover> {
    let doc = Doc::new(text);
    let (line1, at) = doc.scalar_index(enc, pos)?;
    let line = doc.line(line1)?;
    let (word, start, len) = word_at(line, at)?;
    let (_scene, piece) = find_piece(index, text, is_scene, line1, &word)?;
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: piece_markdown(piece),
        }),
        range: Some(doc.range_of(enc, line1, start + 1, len)),
    })
}

/// Go to definition: `Scene: X` jumps to the scene header, a piece name in a
/// step jumps to its line in the scene file.
#[must_use]
pub fn definition(
    index: &Index,
    text: &str,
    is_scene: bool,
    pos: Position,
    enc: Encoding,
    read: &dyn Fn(&Path) -> Option<String>,
) -> Option<GotoDefinitionResponse> {
    let doc = Doc::new(text);
    let (line1, at) = doc.scalar_index(enc, pos)?;
    let line = doc.line(line1)?;
    let (word, _, _) = word_at(line, at)?;

    if line.trim_start().starts_with("Scene:") {
        if is_scene {
            return None;
        }
        let info = index.scenes.get(&word)?;
        return Some(location_of(info, &info.name_span, enc, read)?.into());
    }

    let (scene, piece) = find_piece(index, text, is_scene, line1, &word)?;
    Some(location_of(scene, &piece.name_span, enc, read)?.into())
}

fn location_of(
    scene: &SceneInfo,
    span: &Span,
    enc: Encoding,
    read: &dyn Fn(&Path) -> Option<String>,
) -> Option<Location> {
    let target = read(&span.file)?;
    let target_doc = Doc::new(&target);
    Some(Location {
        uri: scene.uri.clone(),
        range: target_doc.range(enc, span),
    })
}

/// The piece named `word` in the scene that encloses the cursor (a play) or
/// in the scene this file declares (a scene file).
fn find_piece<'a>(
    index: &'a Index,
    text: &str,
    is_scene: bool,
    line1: usize,
    word: &str,
) -> Option<(&'a SceneInfo, &'a PieceInfo)> {
    let scene_name = if is_scene {
        scene_name_of_file(text)?
    } else {
        let lines: Vec<&str> = text.lines().collect();
        let line = lines.get(line1.checked_sub(1)?)?;
        enclosing_scene(&lines, line1 - 1, indent_of(line))?
    };
    let scene = index.scenes.get(&scene_name)?;
    let piece = index.piece(&scene_name, word)?;
    Some((scene, piece))
}

/// The scene name declared by a scene file (`Scene: Name`).
fn scene_name_of_file(text: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(name) = trimmed.strip_prefix("Scene:") {
            let name = name.trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// Document symbols: Play → Act → Stage, and Scene → Pieces.
#[must_use]
pub fn document_symbols(text: &str, path: &Path, enc: Encoding) -> Option<DocumentSymbolResponse> {
    let doc = Doc::new(text);
    let root = if is_scene_file(path) {
        scene_symbol(&doc, enc, &parse_scene(text, path).ast?)
    } else {
        play_symbol(&doc, enc, &parse_play(text, path).ast?)
    };
    Some(DocumentSymbolResponse::Nested(vec![root]))
}

fn play_symbol(doc: &Doc<'_>, enc: Encoding, play: &PlayFile) -> DocumentSymbol {
    let children: Vec<DocumentSymbol> = play
        .acts
        .iter()
        .map(|act| act_symbol(doc, enc, act))
        .collect();
    let last = play.acts.last().map(act_end);
    let mut root = symbol(
        play.name.clone(),
        SymbolKind::MODULE,
        doc,
        enc,
        &play.name_span,
        last,
    );
    root.detail = play.description.clone();
    root.children = Some(children);
    root
}

fn act_symbol(doc: &Doc<'_>, enc: Encoding, act: &ActDecl) -> DocumentSymbol {
    let children: Vec<DocumentSymbol> = act
        .stages
        .iter()
        .map(|stage| stage_symbol(doc, enc, stage))
        .collect();
    let last = act.stages.last().map(stage_end);
    let mut root = symbol(
        act.name.clone(),
        SymbolKind::NAMESPACE,
        doc,
        enc,
        &act.name_span,
        last,
    );
    let tags: Vec<&str> = act.tags.iter().map(|tag| tag.name.as_str()).collect();
    if !tags.is_empty() {
        root.detail = Some(format!("tags: {}", tags.join(", ")));
    }
    root.children = Some(children);
    root
}

fn stage_symbol(doc: &Doc<'_>, enc: Encoding, stage: &StageDecl) -> DocumentSymbol {
    symbol(
        stage.name.clone(),
        SymbolKind::FUNCTION,
        doc,
        enc,
        &stage.name_span,
        Some(stage_end(stage)),
    )
}

fn scene_symbol(doc: &Doc<'_>, enc: Encoding, scene: &SceneFile) -> DocumentSymbol {
    let children: Vec<DocumentSymbol> = scene
        .pieces
        .iter()
        .map(|piece| {
            let mut entry = symbol(
                piece.name.clone(),
                SymbolKind::FIELD,
                doc,
                enc,
                &piece.name_span,
                Some(&piece.span),
            );
            entry.detail = Some(selectors_summary(&piece.selectors));
            entry
        })
        .collect();
    let last = scene.pieces.last().map(|piece| &piece.span);
    let mut root = symbol(
        scene.name.clone(),
        SymbolKind::MODULE,
        doc,
        enc,
        &scene.name_span,
        last,
    );
    root.children = Some(children);
    root
}

/// The last span inside a stage (its last step, else its last scene block).
fn stage_end(stage: &StageDecl) -> &Span {
    stage
        .scenes
        .last()
        .and_then(|block| block.steps.last().map(|step| &step.span))
        .or_else(|| stage.scenes.last().map(|block| &block.scene_span))
        .unwrap_or(&stage.name_span)
}

/// The last span inside an act (its last stage).
fn act_end(act: &ActDecl) -> &Span {
    act.stages.last().map_or(&act.name_span, stage_end)
}

fn symbol(
    name: String,
    kind: SymbolKind,
    doc: &Doc<'_>,
    enc: Encoding,
    name_span: &Span,
    last: Option<&Span>,
) -> DocumentSymbol {
    #[allow(deprecated)] // the field is deprecated in favour of tags; nothing to tag here
    DocumentSymbol {
        name,
        detail: None,
        kind,
        tags: None,
        deprecated: None,
        range: range_covering(doc, enc, name_span, last),
        selection_range: doc.range(enc, name_span),
        children: None,
    }
}

fn range_covering(doc: &Doc<'_>, enc: Encoding, start: &Span, last: Option<&Span>) -> Range {
    let start_pos = doc.position(enc, start.line, start.col);
    let end_pos = match last {
        Some(span) => doc.position(enc, span.line, span.col.saturating_add(span.len)),
        None => doc.position(enc, start.line, start.col.saturating_add(start.len)),
    };
    Range::new(start_pos, end_pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCENE: &str = r##"Scene: LoginScreen
Platform: cross-platform

Pieces:
    LoginButton: android="id:login_button", web="#login"
    UsernameInput: android="id:username"
"##;

    const PLAY: &str = r#"Play: Login
Description: sign in

Act: Successful Login
Tags: smoke

Stage: Navigate
    Scene: LoginScreen
        Action: click => LoginButton
        Check: UsernameInput
"#;

    fn index() -> Index {
        let scene_path = std::env::temp_dir().join("impresario-lsp/scenes/LoginScreen.scene");
        let play_path = std::env::temp_dir().join("impresario-lsp/plays/login.play");
        let scene = parse_scene(SCENE, &scene_path).ast.expect("scene");
        let play = parse_play(PLAY, &play_path).ast.expect("play");
        Index::build(&[scene], &[play])
    }

    fn reader() -> impl Fn(&Path) -> Option<String> {
        |path: &Path| {
            if path.ends_with("LoginScreen.scene") {
                Some(SCENE.to_string())
            } else {
                None
            }
        }
    }

    #[test]
    fn hover_on_a_piece_shows_the_selector_table() {
        let index = index();
        let hover = hover(&index, PLAY, false, Position::new(8, 27), Encoding::Utf16)
            .expect("hover on LoginButton");
        let HoverContents::Markup(markup) = hover.contents else {
            panic!("markdown hover");
        };
        assert_eq!(markup.kind, MarkupKind::Markdown);
        assert!(markup.value.contains("**LoginButton**"), "{}", markup.value);
        assert!(markup.value.contains("| android | `id:login_button` |"));
        assert!(markup.value.contains("| web | `#login` |"));
    }

    #[test]
    fn hover_on_a_word_that_is_not_a_piece_is_none() {
        let index = index();
        assert!(hover(&index, PLAY, false, Position::new(0, 3), Encoding::Utf16).is_none());
    }

    #[test]
    fn definition_of_a_scene_reference_is_the_scene_header() {
        let index = index();
        let read = reader();
        let found = definition(
            &index,
            PLAY,
            false,
            Position::new(7, 18),
            Encoding::Utf16,
            &read,
        )
        .expect("definition");
        let GotoDefinitionResponse::Scalar(location) = found else {
            panic!("scalar location");
        };
        assert!(location.uri.path().as_str().ends_with("LoginScreen.scene"));
        assert_eq!(location.range.start.line, 0);
        assert_eq!(location.range.start.character, 7); // after "Scene: "
    }

    #[test]
    fn definition_of_a_piece_is_its_declaration_line() {
        let index = index();
        let read = reader();
        let found = definition(
            &index,
            PLAY,
            false,
            Position::new(8, 30),
            Encoding::Utf16,
            &read,
        )
        .expect("definition of LoginButton");
        let GotoDefinitionResponse::Scalar(location) = found else {
            panic!("scalar location");
        };
        assert!(location.uri.path().as_str().ends_with("LoginScreen.scene"));
        assert_eq!(location.range.start.line, 4);
        assert_eq!(location.range.start.character, 4);
        assert_eq!(location.range.end.character, 15); // "LoginButton"
    }

    #[test]
    fn document_symbols_outline_play_and_scene() {
        let play = document_symbols(PLAY, Path::new("plays/login.play"), Encoding::Utf16)
            .expect("play symbols");
        let DocumentSymbolResponse::Nested(roots) = play else {
            panic!("nested symbols");
        };
        let root = &roots[0];
        assert_eq!(root.name, "Login");
        assert_eq!(root.kind, SymbolKind::MODULE);
        let acts = root.children.as_ref().expect("acts");
        assert_eq!(acts[0].name, "Successful Login");
        assert_eq!(acts[0].detail.as_deref(), Some("tags: smoke"));
        let stages = acts[0].children.as_ref().expect("stages");
        assert_eq!(stages[0].name, "Navigate");
        assert_eq!(stages[0].kind, SymbolKind::FUNCTION);
        // The stage range must cover its steps and contain its selection.
        let range = stages[0].range;
        assert!(range.start.line < range.end.line);
        assert!(range.start <= stages[0].selection_range.start);

        let scene = document_symbols(
            SCENE,
            Path::new("scenes/LoginScreen.scene"),
            Encoding::Utf16,
        )
        .expect("scene symbols");
        let DocumentSymbolResponse::Nested(roots) = scene else {
            panic!("nested symbols");
        };
        let pieces = roots[0].children.as_ref().expect("pieces");
        assert_eq!(pieces.len(), 2);
        assert_eq!(pieces[0].name, "LoginButton");
        assert_eq!(
            pieces[0].detail.as_deref(),
            Some("android=\"id:login_button\", web=\"#login\"")
        );
    }

    #[test]
    fn broken_files_have_no_symbols() {
        assert!(
            document_symbols("totally not a scene", Path::new("x.scene"), Encoding::Utf16)
                .is_none()
        );
    }
}
