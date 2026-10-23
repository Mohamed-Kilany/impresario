//! The shared editor corpus (`fixtures/lang-corpus`).
//!
//! The LSP suite and the standalone WASM extension run the exact same files
//! and expectations; this test is the Rust half of the parity gate.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use impresario_core::apps::{parse_apps, AppConfig};
use impresario_core::config::{parse_devices, DeviceConfig};
use impresario_core::validation::DeviceRef;
use impresario_language::model::{AppInfo, SourceFile};
use impresario_language::service::{DocumentSymbol, Language};
use impresario_language::text::Position;
use serde_json::{json, Value};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/lang-corpus")
}

fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("corpus directory") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            collect(&path, files);
        } else if path.extension().is_some_and(|extension| {
            matches!(extension.to_str(), Some("scene" | "theatre" | "play"))
        }) {
            files.push(path);
        }
    }
}

fn relative(path: &Path) -> String {
    path.strip_prefix(root().join("files"))
        .expect("corpus file below files/")
        .to_string_lossy()
        .replace('\\', "/")
}

fn sources() -> Vec<SourceFile> {
    let mut files = Vec::new();
    collect(&root().join("files"), &mut files);
    files.sort();
    files
        .into_iter()
        .map(|path| SourceFile {
            text: std::fs::read_to_string(&path).expect("read corpus file"),
            path: PathBuf::from(relative(&path)),
        })
        .collect()
}

fn devices() -> Vec<DeviceRef> {
    let path = root().join("devices.config");
    let source = std::fs::read_to_string(&path).expect("read devices");
    let outcome = parse_devices(&source, &path);
    assert!(
        outcome.diagnostics.is_empty(),
        "corpus devices must be valid: {:?}",
        outcome.diagnostics
    );
    outcome
        .config
        .expect("devices")
        .devices
        .iter()
        .map(device_ref)
        .collect()
}

fn device_ref(device: &DeviceConfig) -> DeviceRef {
    DeviceRef::new(device.name.clone(), device.platform)
}

fn apps() -> Vec<AppInfo> {
    let path = root().join("apps.config");
    let source = std::fs::read_to_string(&path).expect("read apps");
    let outcome = parse_apps(&source, &path);
    assert!(
        outcome.diagnostics.is_empty(),
        "corpus apps must be valid: {:?}",
        outcome.diagnostics
    );
    outcome
        .config
        .expect("apps")
        .apps
        .iter()
        .map(app_info)
        .collect()
}

fn app_info(app: &AppConfig) -> AppInfo {
    AppInfo {
        name: app.name.clone(),
        description: app.description.clone(),
    }
}

fn language() -> Language {
    Language::analyze_with_apps(&sources(), &devices(), &apps(), Some(root().join("files")))
}

fn normalized(info: &impresario_language::diagnostics::DiagnosticInfo) -> Value {
    json!({
        "code": info.code,
        "severity": info.severity,
        "line": info.span.line,
        "col": info.span.col,
        "message": info.message,
        "related": info.related.iter().map(|related| json!({
            "line": related.span.line,
            "col": related.span.col,
            "message": related.message,
        })).collect::<Vec<_>>(),
    })
}

#[test]
fn diagnostics_match_the_shared_expectations() {
    let language = language();
    let expected: Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("expected-diagnostics.json")).expect("diagnostics"),
    )
    .expect("diagnostics json");
    let expected = expected.as_object().expect("diagnostics object");

    for source in sources() {
        let key = source.path.to_string_lossy().replace('\\', "/");
        let actual = language
            .diagnostics_for(&source.path)
            .iter()
            .map(normalized)
            .collect::<Vec<_>>();
        assert_eq!(
            Value::Array(actual),
            expected.get(&key).cloned().unwrap_or(Value::Null),
            "diagnostics differ for {key}"
        );
    }
    for key in expected.keys() {
        assert!(
            sources()
                .iter()
                .any(|source| source.path.to_string_lossy().replace('\\', "/") == *key),
            "expectation for missing corpus file {key}"
        );
    }
}

#[test]
fn every_source_formats_to_the_shared_expected_text() {
    let language = language();
    for source in sources() {
        let relative = source.path.to_string_lossy().replace('\\', "/");
        let expected = std::fs::read_to_string(root().join("expected-format").join(&relative))
            .unwrap_or_else(|_| panic!("missing expected-format/{relative}"));
        assert_eq!(
            language.formatted_text(&source.path),
            Some(expected),
            "format differs for {relative}"
        );
    }
}

fn symbol_names(symbols: &[DocumentSymbol], names: &mut Vec<String>) {
    for symbol in symbols {
        names.push(symbol.name.clone());
        symbol_names(&symbol.children, names);
    }
}

#[test]
fn feature_probes_match_the_shared_expectations() {
    let language = language();
    let probes: Vec<Value> = serde_json::from_str(
        &std::fs::read_to_string(root().join("feature-probes.json")).expect("features"),
    )
    .expect("feature probes json");
    for probe in probes {
        let op = probe["op"].as_str().expect("op");
        let path = probe["path"]
            .as_str()
            .map(PathBuf::from)
            .unwrap_or_default();
        let position = || {
            Position::new(
                u32::try_from(probe["line"].as_u64().expect("line")).expect("line"),
                u32::try_from(probe["col"].as_u64().expect("col")).expect("col"),
            )
        };
        match op {
            "hover" => {
                let hover = language.hover(&path, position()).expect("hover");
                for needle in probe["contains"].as_array().expect("contains") {
                    let needle = needle.as_str().expect("needle");
                    assert!(
                        hover.markdown.contains(needle),
                        "{} hover lacks {needle:?}: {}",
                        path.display(),
                        hover.markdown
                    );
                }
            }
            "definition" => {
                let spans = language.definition(&path, position());
                let expected = probe["expect"].as_array().expect("expect");
                assert_eq!(spans.len(), expected.len(), "{spans:?}");
                for (span, expected) in spans.iter().zip(expected) {
                    assert_eq!(
                        span.file,
                        PathBuf::from(expected["path"].as_str().expect("path"))
                    );
                    assert_eq!(
                        span.line,
                        usize::try_from(expected["line"].as_u64().expect("line")).expect("line")
                    );
                    assert_eq!(
                        span.col,
                        usize::try_from(expected["col"].as_u64().expect("col")).expect("col")
                    );
                }
            }
            "references" => {
                let spans = language.references(&path, position(), true);
                assert!(
                    spans.len()
                        >= usize::try_from(probe["min"].as_u64().expect("min")).expect("min"),
                    "{spans:?}"
                );
                for expected in probe["expect_contains"].as_array().expect("contains") {
                    let file = expected["path"].as_str().expect("path");
                    let line =
                        usize::try_from(expected["line"].as_u64().expect("line")).expect("line");
                    let col = usize::try_from(expected["col"].as_u64().expect("col")).expect("col");
                    assert!(
                        spans.iter().any(|span| {
                            span.file.as_path() == Path::new(file)
                                && span.line == line
                                && span.col == col
                        }),
                        "references lack {file}:{line}:{col}: {spans:?}"
                    );
                }
            }
            "prepareRename" => {
                let prepared = language.prepare_rename(&path, position()).expect("prepare");
                assert_eq!(
                    prepared.placeholder,
                    probe["placeholder"].as_str().expect("placeholder")
                );
            }
            "rename" => {
                let edit = language
                    .rename(
                        &path,
                        position(),
                        probe["new_name"].as_str().expect("new name"),
                    )
                    .expect("rename");
                let changed = edit.changes.keys().cloned().collect::<BTreeSet<_>>();
                let expected = probe["changed_files"]
                    .as_array()
                    .expect("changed files")
                    .iter()
                    .map(|file| file.as_str().expect("file").to_string())
                    .collect::<BTreeSet<_>>();
                assert_eq!(changed, expected, "{changed:?}");
                let count = edit.changes.values().map(Vec::len).sum::<usize>();
                assert_eq!(
                    count,
                    usize::try_from(probe["expected_edits"].as_u64().expect("edits"))
                        .expect("edits")
                );
                let new_name = probe["new_name"].as_str().expect("new name");
                assert!(edit
                    .changes
                    .values()
                    .flatten()
                    .all(|edit| edit.new_text == new_name));
            }
            "documentSymbols" => {
                let mut names = Vec::new();
                symbol_names(&language.document_symbols(&path), &mut names);
                for expected in probe["contains"].as_array().expect("contains") {
                    let expected = expected.as_str().expect("name");
                    assert!(names.iter().any(|name| name == expected), "{names:?}");
                }
            }
            "workspaceSymbols" => {
                let names = language
                    .workspace_symbols(probe["query"].as_str().expect("query"))
                    .into_iter()
                    .map(|symbol| symbol.name)
                    .collect::<Vec<_>>();
                for expected in probe["contains"].as_array().expect("contains") {
                    let expected = expected.as_str().expect("name");
                    assert!(names.iter().any(|name| name == expected), "{names:?}");
                }
            }
            "folding" => {
                let ranges = language.folding_ranges(&path);
                assert!(
                    ranges.len()
                        >= usize::try_from(probe["min"].as_u64().expect("min")).expect("min"),
                    "{ranges:?}"
                );
            }
            "semanticTokens" => {
                let tokens = language.semantic_tokens(&path);
                assert!(
                    tokens.len()
                        >= usize::try_from(probe["min"].as_u64().expect("min")).expect("min"),
                    "{tokens:?}"
                );
                for expected in probe["contains_types"].as_array().expect("types") {
                    let expected = expected.as_str().expect("type");
                    assert!(
                        tokens.iter().any(|token| token.token_type == expected),
                        "{tokens:?}"
                    );
                }
            }
            "signatureHelp" => {
                let help = language
                    .signature_help(&path, position())
                    .expect("signature");
                let label = probe["label_contains"].as_str().expect("label");
                assert!(help
                    .signatures
                    .iter()
                    .any(|signature| signature.label.contains(label)));
                assert_eq!(
                    help.active_parameter,
                    u32::try_from(probe["active_parameter"].as_u64().expect("active"))
                        .expect("active")
                );
            }
            "codeActions" => {
                let start = Position::new(
                    u32::try_from(probe["start"]["line"].as_u64().expect("line")).expect("line"),
                    u32::try_from(probe["start"]["col"].as_u64().expect("col")).expect("col"),
                );
                let end = Position::new(
                    u32::try_from(probe["end"]["line"].as_u64().expect("line")).expect("line"),
                    u32::try_from(probe["end"]["col"].as_u64().expect("col")).expect("col"),
                );
                let actions = language.code_actions(&path, start, end);
                let titles = actions
                    .iter()
                    .map(|action| action.title.as_str())
                    .collect::<Vec<_>>();
                for expected in probe["contains_titles"].as_array().expect("titles") {
                    let expected = expected.as_str().expect("title");
                    assert!(titles.contains(&expected), "{titles:?}");
                }
            }
            "inlayHints" => {
                let line = u32::try_from(probe["line"].as_u64().expect("line")).expect("line");
                let hints = language.inlay_hints(&path, Some((line, line)));
                for expected in probe["contains"].as_array().expect("contains") {
                    let expected = expected.as_str().expect("label");
                    assert!(
                        hints.iter().any(|hint| hint.label.contains(expected)),
                        "{hints:?}"
                    );
                }
            }
            "codeLenses" => {
                let lenses = language.code_lenses(&path);
                for expected in probe["contains"].as_array().expect("contains") {
                    let expected = expected.as_str().expect("title");
                    assert!(
                        lenses
                            .iter()
                            .any(|lens| lens.command.title.contains(expected)),
                        "{lenses:?}"
                    );
                }
            }
            "rangeFormatting" => {
                let start = Position::new(
                    u32::try_from(probe["start"]["line"].as_u64().expect("line")).expect("line"),
                    u32::try_from(probe["start"]["col"].as_u64().expect("col")).expect("col"),
                );
                let end = Position::new(
                    u32::try_from(probe["end"]["line"].as_u64().expect("line")).expect("line"),
                    u32::try_from(probe["end"]["col"].as_u64().expect("col")).expect("col"),
                );
                let edits = language
                    .format_range(&path, start, end)
                    .expect("range format");
                let expected = probe["new_text_contains"].as_str().expect("new text");
                assert!(
                    edits.iter().any(|edit| edit.new_text.contains(expected)),
                    "{edits:?}"
                );
            }
            "onTypeFormatting" => {
                let ch = probe["ch"]
                    .as_str()
                    .and_then(|ch| ch.chars().next())
                    .expect("ch");
                let edits = language
                    .format_on_type(&path, position(), ch)
                    .expect("on type");
                let expected = probe["new_text_contains"].as_str().expect("new text");
                assert!(
                    edits.iter().any(|edit| edit.new_text.contains(expected)),
                    "{edits:?}"
                );
            }
            other => panic!("unknown feature probe {other}"),
        }
    }
}

#[test]
fn completion_probes_match_the_shared_expectations() {
    let language = language();
    let probes: Vec<Value> = serde_json::from_str(
        &std::fs::read_to_string(root().join("completion-probes.json")).expect("probes"),
    )
    .expect("probes json");
    for probe in probes {
        let path = PathBuf::from(probe["path"].as_str().expect("probe path"));
        let line = u32::try_from(probe["line"].as_u64().expect("probe line")).expect("line");
        let col = u32::try_from(probe["col"].as_u64().expect("probe col")).expect("col");
        let labels = language
            .completion(&path, Position::new(line, col))
            .into_iter()
            .map(|item| item.label)
            .collect::<Vec<_>>();
        for expected in probe["contains"].as_array().expect("contains") {
            let expected = expected.as_str().expect("label");
            assert!(
                labels.iter().any(|label| label == expected),
                "{}:{line}:{col} lacks {expected:?}; got {labels:?}",
                path.display()
            );
        }
    }
}
