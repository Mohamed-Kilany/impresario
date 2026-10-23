//! Conversions between the editor-neutral language service and LSP types.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use impresario_core::span::Span;
use impresario_language::service as ls;
use impresario_language::text::Position as ServicePosition;
use tower_lsp_server::ls_types as lsp;

use super::position::{Doc, Encoding};

/// Semantic-token legend shared by the capability and the encoder.
pub const TOKEN_TYPES: &[&str] = &[
    "namespace",
    "function",
    "property",
    "variable",
    "class",
    "keyword",
];
/// Semantic-token modifiers.
pub const TOKEN_MODIFIERS: &[&str] = &["declaration"];

pub fn position(
    text: &str,
    encoding: Encoding,
    position: lsp::Position,
) -> Option<ServicePosition> {
    let doc = Doc::new(text);
    let (line, index) = doc.scalar_index(encoding, position)?;
    Some(ServicePosition::new(
        u32::try_from(line).ok()?,
        u32::try_from(index).ok()?.saturating_add(1),
    ))
}

/// The LSP range of a core span.
///
/// Most spans are single-line, but formatter edits cover a whole line block,
/// so `len` is walked across line breaks instead of being treated as one
/// giant column.
pub fn range(text: &str, encoding: Encoding, span: &Span) -> lsp::Range {
    let doc = Doc::new(text);
    let mut line = span.line.max(1);
    let mut col = span.col.max(1);
    let mut remaining = span.len;
    while remaining > 0 {
        let content = doc.line(line).unwrap_or("");
        let length = content.chars().count();
        let available = length.saturating_sub(col.saturating_sub(1));
        if remaining <= available {
            col = col.saturating_add(remaining);
            remaining = 0;
        } else {
            remaining -= available;
            if remaining == 0 {
                break;
            }
            // The newline between the two lines is part of the span.
            remaining -= 1;
            line = line.saturating_add(1);
            col = 1;
        }
    }
    lsp::Range::new(
        doc.position(encoding, span.line, span.col),
        doc.position(encoding, line, col),
    )
}

pub fn location(
    sources: &BTreeMap<PathBuf, String>,
    encoding: Encoding,
    span: &Span,
) -> Option<lsp::Location> {
    let text = sources.get(&span.file)?;
    Some(lsp::Location {
        uri: lsp::Uri::from_file_path(&span.file)?,
        range: range(text, encoding, span),
    })
}

pub fn text_edit(
    sources: &BTreeMap<PathBuf, String>,
    encoding: Encoding,
    edit: &ls::TextEdit,
) -> Option<lsp::TextEdit> {
    let text = sources.get(&edit.span.file).map_or("", String::as_str);
    Some(lsp::TextEdit {
        range: range(text, encoding, &edit.span),
        new_text: edit.new_text.clone(),
    })
}

pub fn completion_item(
    sources: &BTreeMap<PathBuf, String>,
    encoding: Encoding,
    item: ls::CompletionItem,
) -> lsp::CompletionItem {
    lsp::CompletionItem {
        label: item.label,
        kind: Some(completion_kind(&item.kind)),
        detail: item.detail,
        documentation: item.documentation.map(lsp::Documentation::String),
        sort_text: item.sort_text,
        filter_text: item.filter_text,
        insert_text: item.insert_text,
        text_edit: item
            .replace
            .as_ref()
            .and_then(|edit| text_edit(sources, encoding, edit))
            .map(lsp::CompletionTextEdit::Edit),
        ..lsp::CompletionItem::default()
    }
}

fn completion_kind(kind: &str) -> lsp::CompletionItemKind {
    match kind {
        "file" => lsp::CompletionItemKind::FILE,
        "function" => lsp::CompletionItemKind::FUNCTION,
        "property" => lsp::CompletionItemKind::PROPERTY,
        "reference" => lsp::CompletionItemKind::REFERENCE,
        "constant" => lsp::CompletionItemKind::CONSTANT,
        "class" => lsp::CompletionItemKind::CLASS,
        "value" => lsp::CompletionItemKind::VALUE,
        "variable" => lsp::CompletionItemKind::VARIABLE,
        "keyword" => lsp::CompletionItemKind::KEYWORD,
        "namespace" => lsp::CompletionItemKind::MODULE,
        "enumMember" => lsp::CompletionItemKind::ENUM_MEMBER,
        "operator" => lsp::CompletionItemKind::OPERATOR,
        "snippet" => lsp::CompletionItemKind::SNIPPET,
        _ => lsp::CompletionItemKind::TEXT,
    }
}

pub fn hover(text: &str, encoding: Encoding, hover: ls::HoverResult) -> lsp::Hover {
    lsp::Hover {
        contents: lsp::HoverContents::Markup(lsp::MarkupContent {
            kind: lsp::MarkupKind::Markdown,
            value: hover.markdown,
        }),
        range: hover.span.as_ref().map(|span| range(text, encoding, span)),
    }
}

fn symbol_kind(kind: &str) -> lsp::SymbolKind {
    match kind {
        "namespace" => lsp::SymbolKind::NAMESPACE,
        "function" => lsp::SymbolKind::FUNCTION,
        "property" => lsp::SymbolKind::PROPERTY,
        "variable" => lsp::SymbolKind::VARIABLE,
        "class" => lsp::SymbolKind::CLASS,
        _ => lsp::SymbolKind::OBJECT,
    }
}

#[allow(deprecated)]
pub fn document_symbol(
    sources: &BTreeMap<PathBuf, String>,
    encoding: Encoding,
    symbol: &ls::DocumentSymbol,
) -> Option<lsp::DocumentSymbol> {
    let text = sources.get(&symbol.span.file).map_or("", String::as_str);
    Some(lsp::DocumentSymbol {
        name: symbol.name.clone(),
        detail: symbol.detail.clone(),
        kind: symbol_kind(&symbol.kind),
        tags: None,
        deprecated: None,
        range: range(text, encoding, &symbol.span),
        selection_range: range(text, encoding, &symbol.selection_span),
        children: Some(
            symbol
                .children
                .iter()
                .filter_map(|child| document_symbol(sources, encoding, child))
                .collect(),
        ),
    })
}

pub fn workspace_symbol(
    sources: &BTreeMap<PathBuf, String>,
    encoding: Encoding,
    symbol: &ls::WorkspaceSymbol,
) -> Option<lsp::WorkspaceSymbol> {
    let text = sources.get(&symbol.span.file).map_or("", String::as_str);
    Some(lsp::WorkspaceSymbol {
        name: symbol.name.clone(),
        kind: symbol_kind(&symbol.kind),
        tags: None,
        container_name: symbol.detail.clone(),
        location: lsp::OneOf::Left(lsp::Location {
            uri: lsp::Uri::from_file_path(&symbol.span.file)?,
            range: range(text, encoding, &symbol.span),
        }),
        data: None,
    })
}

pub fn folding(folding: &ls::FoldingRange) -> lsp::FoldingRange {
    lsp::FoldingRange {
        start_line: folding.start_line.saturating_sub(1),
        start_character: None,
        end_line: folding.end_line.saturating_sub(1),
        end_character: None,
        kind: folding.kind.as_deref().and_then(|kind| match kind {
            "comment" => Some(lsp::FoldingRangeKind::Comment),
            "imports" => Some(lsp::FoldingRangeKind::Imports),
            "region" => Some(lsp::FoldingRangeKind::Region),
            _ => None,
        }),
        collapsed_text: None,
    }
}

pub fn semantic_tokens(
    sources: &BTreeMap<PathBuf, String>,
    encoding: Encoding,
    tokens: &[ls::SemanticToken],
    selection: Option<lsp::Range>,
) -> lsp::SemanticTokens {
    let mut data = Vec::new();
    let mut previous_line = 0u32;
    let mut previous_start = 0u32;
    for token in tokens {
        let Some(text) = sources.get(&token.span.file) else {
            continue;
        };
        let doc = Doc::new(text);
        let range = doc.range(encoding, &token.span);
        if range.start.line != range.end.line {
            continue;
        }
        if let Some(selection) = selection {
            if range.end.line < selection.start.line || range.start.line > selection.end.line {
                continue;
            }
        }
        let length = range.end.character.saturating_sub(range.start.character);
        if length == 0 {
            continue;
        }
        let token_type = TOKEN_TYPES
            .iter()
            .position(|candidate| *candidate == token.token_type)
            .unwrap_or(TOKEN_TYPES.len().saturating_sub(1));
        let mut modifiers = 0u32;
        for modifier in &token.modifiers {
            if let Some(index) = TOKEN_MODIFIERS
                .iter()
                .position(|candidate| *candidate == modifier)
            {
                modifiers |= 1 << index;
            }
        }
        let delta_line = range.start.line.saturating_sub(previous_line);
        let delta_start = if delta_line == 0 {
            range.start.character.saturating_sub(previous_start)
        } else {
            range.start.character
        };
        data.push(lsp::SemanticToken {
            delta_line,
            delta_start,
            length,
            token_type: u32::try_from(token_type).unwrap_or(0),
            token_modifiers_bitset: modifiers,
        });
        previous_line = range.start.line;
        previous_start = range.start.character;
    }
    lsp::SemanticTokens {
        result_id: None,
        data,
    }
}

pub fn signature_help(help: &ls::SignatureHelp) -> lsp::SignatureHelp {
    lsp::SignatureHelp {
        signatures: help
            .signatures
            .iter()
            .map(|signature| lsp::SignatureInformation {
                label: signature.label.clone(),
                documentation: signature
                    .documentation
                    .clone()
                    .map(lsp::Documentation::String),
                parameters: Some(
                    signature
                        .parameters
                        .iter()
                        .map(|parameter| lsp::ParameterInformation {
                            label: lsp::ParameterLabel::Simple(parameter.label.clone()),
                            documentation: parameter
                                .documentation
                                .clone()
                                .map(lsp::Documentation::String),
                        })
                        .collect(),
                ),
                active_parameter: None,
            })
            .collect(),
        active_signature: Some(help.active_signature),
        active_parameter: Some(help.active_parameter),
    }
}

pub fn inlay_hint(
    sources: &BTreeMap<PathBuf, String>,
    encoding: Encoding,
    hint: &ls::InlayHint,
) -> Option<lsp::InlayHint> {
    let text = sources.get(&hint.span.file)?;
    Some(lsp::InlayHint {
        position: Doc::new(text).position(encoding, hint.span.line, hint.span.col),
        label: lsp::InlayHintLabel::String(hint.label.clone()),
        kind: Some(match hint.kind.as_str() {
            "type" => lsp::InlayHintKind::TYPE,
            _ => lsp::InlayHintKind::PARAMETER,
        }),
        text_edits: None,
        tooltip: None,
        padding_left: None,
        padding_right: None,
        data: None,
    })
}

pub fn code_lens(
    sources: &BTreeMap<PathBuf, String>,
    encoding: Encoding,
    lens: &ls::CodeLens,
) -> Option<lsp::CodeLens> {
    let text = sources.get(&lens.span.file)?;
    Some(lsp::CodeLens {
        range: range(text, encoding, &lens.span),
        command: Some(command(&lens.command)),
        data: None,
    })
}

pub fn command(command: &ls::Command) -> lsp::Command {
    lsp::Command {
        title: command.title.clone(),
        command: command.command.clone(),
        arguments: Some(command.arguments.clone()),
    }
}

pub fn code_action(
    sources: &BTreeMap<PathBuf, String>,
    encoding: Encoding,
    action: &ls::CodeAction,
) -> Option<lsp::CodeAction> {
    Some(lsp::CodeAction {
        title: action.title.clone(),
        kind: Some(lsp::CodeActionKind::from(action.kind.clone())),
        diagnostics: None,
        edit: action
            .edit
            .as_ref()
            .and_then(|edit| workspace_edit(sources, encoding, edit)),
        command: action.command.as_ref().map(command),
        is_preferred: None,
        disabled: None,
        data: None,
    })
}

pub fn workspace_edit(
    sources: &BTreeMap<PathBuf, String>,
    encoding: Encoding,
    edit: &ls::WorkspaceEdit,
) -> Option<lsp::WorkspaceEdit> {
    let has_file_ops = !edit.create_files.is_empty()
        || !edit.delete_files.is_empty()
        || !edit.rename_files.is_empty();
    if !has_file_ops {
        let mut changes = HashMap::new();
        for (path, edits) in &edit.changes {
            let uri = lsp::Uri::from_file_path(path)?;
            changes.insert(
                uri,
                edits
                    .iter()
                    .filter_map(|edit| text_edit(sources, encoding, edit))
                    .collect(),
            );
        }
        return Some(lsp::WorkspaceEdit {
            changes: Some(changes),
            ..lsp::WorkspaceEdit::default()
        });
    }

    let mut operations = Vec::new();
    for file in &edit.create_files {
        let uri = lsp::Uri::from_file_path(&file.path)?;
        operations.push(lsp::DocumentChangeOperation::Op(lsp::ResourceOp::Create(
            lsp::CreateFile {
                uri: uri.clone(),
                options: Some(lsp::CreateFileOptions {
                    overwrite: Some(file.overwrite),
                    ignore_if_exists: Some(false),
                }),
                annotation_id: None,
            },
        )));
        let end = position_at_end(&file.text, encoding);
        operations.push(lsp::DocumentChangeOperation::Edit(lsp::TextDocumentEdit {
            text_document: lsp::OptionalVersionedTextDocumentIdentifier { uri, version: None },
            edits: vec![lsp::OneOf::Left(lsp::TextEdit {
                range: lsp::Range::new(lsp::Position::new(0, 0), end),
                new_text: file.text.clone(),
            })],
        }));
    }
    for path in &edit.delete_files {
        let uri = lsp::Uri::from_file_path(path)?;
        operations.push(lsp::DocumentChangeOperation::Op(lsp::ResourceOp::Delete(
            lsp::DeleteFile {
                uri,
                options: Some(lsp::DeleteFileOptions {
                    recursive: Some(false),
                    ignore_if_not_exists: Some(true),
                }),
                annotation_id: None,
            },
        )));
    }
    for rename in &edit.rename_files {
        operations.push(lsp::DocumentChangeOperation::Op(lsp::ResourceOp::Rename(
            lsp::RenameFile {
                old_uri: lsp::Uri::from_file_path(&rename.old_path)?,
                new_uri: lsp::Uri::from_file_path(&rename.new_path)?,
                options: Some(lsp::RenameFileOptions {
                    overwrite: Some(false),
                    ignore_if_exists: Some(false),
                }),
                annotation_id: None,
            },
        )));
    }
    for (path, edits) in &edit.changes {
        let uri = lsp::Uri::from_file_path(path)?;
        operations.push(lsp::DocumentChangeOperation::Edit(lsp::TextDocumentEdit {
            text_document: lsp::OptionalVersionedTextDocumentIdentifier { uri, version: None },
            edits: edits
                .iter()
                .filter_map(|edit| text_edit(sources, encoding, edit))
                .map(lsp::OneOf::Left)
                .collect(),
        }));
    }
    Some(lsp::WorkspaceEdit {
        document_changes: Some(lsp::DocumentChanges::Operations(operations)),
        ..lsp::WorkspaceEdit::default()
    })
}

fn position_at_end(text: &str, encoding: Encoding) -> lsp::Position {
    let lines: Vec<&str> = text.split('\n').collect();
    let line = lines.len().saturating_sub(1);
    let last = lines.last().copied().unwrap_or_default();
    let character = match encoding {
        Encoding::Utf8 => last.len(),
        Encoding::Utf16 => last.encode_utf16().count(),
    };
    lsp::Position::new(
        u32::try_from(line).unwrap_or(u32::MAX),
        u32::try_from(character).unwrap_or(u32::MAX),
    )
}
