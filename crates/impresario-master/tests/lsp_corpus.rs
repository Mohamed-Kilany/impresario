//! The embedded LSP against `fixtures/lang-corpus`.
//!
//! This is the LSP half of the parity gate: diagnostics (including
//! related-information duplicates), canonical formatting and completion
//! probes are compared with the exact expectations the Rust service and the
//! standalone WASM extension use.

mod common;

use std::path::{Path, PathBuf};

use common::lsp::{self, TestLsp};
use serde_json::{json, Value};

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

fn corpus_root() -> PathBuf {
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

fn corpus_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect(&corpus_root().join("files"), &mut files);
    files.sort();
    files
}

fn relative(path: &Path) -> String {
    path.strip_prefix(corpus_root().join("files"))
        .expect("corpus file below files/")
        .to_string_lossy()
        .replace('\\', "/")
}

fn copy_corpus(base: &Path) -> Vec<(PathBuf, String)> {
    let mut copied = Vec::new();
    for source in corpus_files() {
        let key = relative(&source);
        let target = base.join(key.replace('/', std::path::MAIN_SEPARATOR_STR));
        lsp::write(
            &target,
            &std::fs::read_to_string(&source).expect("read corpus file"),
        );
        copied.push((target, key));
    }
    lsp::write(&base.join("impresario.config"), "project: lang_corpus\n");
    lsp::write(
        &base.join("impresario.devices.config"),
        &std::fs::read_to_string(corpus_root().join("devices.config")).expect("devices"),
    );
    lsp::write(
        &base.join("impresario.apps.config"),
        &std::fs::read_to_string(corpus_root().join("apps.config")).expect("apps"),
    );
    copied
}

fn normalize_message(message: &str, base: &Path, copied: &[(PathBuf, String)]) -> String {
    let mut message = message
        .split("\nhelp: ")
        .next()
        .unwrap_or(message)
        .to_string();
    let base_display = base.display().to_string();
    let base_forward = base_display.replace('\\', "/");
    for (path, key) in copied {
        let absolute = path.display().to_string();
        let forward = absolute.replace('\\', "/");
        message = message.replace(&absolute, key).replace(&forward, key);
    }
    message = message
        .replace(&base_display, "")
        .replace(&base_forward, "");
    message.replace('\\', "/")
}

fn normalize_diagnostics(
    params: Option<&Value>,
    base: &Path,
    copied: &[(PathBuf, String)],
) -> Vec<Value> {
    let Some(diagnostics) = params
        .and_then(|params| params.get("diagnostics"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let mut normalized = diagnostics
        .iter()
        .map(|diagnostic| {
            let severity = match diagnostic["severity"].as_u64() {
                Some(1) => "error",
                Some(2) => "warning",
                _ => "info",
            };
            let mut related = diagnostic["relatedInformation"]
                .as_array()
                .map(|related| {
                    related
                        .iter()
                        .map(|related| {
                            json!({
                                "line": related["location"]["range"]["start"]["line"].as_u64().unwrap_or(0) + 1,
                                "col": related["location"]["range"]["start"]["character"].as_u64().unwrap_or(0) + 1,
                                "message": normalize_message(
                                    related["message"].as_str().unwrap_or(""),
                                    base,
                                    copied,
                                ),
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            related.sort_by(|left, right| {
                (left["line"].as_u64(), left["col"].as_u64(), left["message"].as_str()).cmp(&(
                    right["line"].as_u64(),
                    right["col"].as_u64(),
                    right["message"].as_str(),
                ))
            });
            json!({
                "code": diagnostic["code"].as_str().unwrap_or(""),
                "severity": severity,
                "line": diagnostic["range"]["start"]["line"].as_u64().unwrap_or(0) + 1,
                "col": diagnostic["range"]["start"]["character"].as_u64().unwrap_or(0) + 1,
                "message": normalize_message(
                    diagnostic["message"].as_str().unwrap_or(""),
                    base,
                    copied,
                ),
                "related": related,
            })
        })
        .collect::<Vec<_>>();
    sort_diagnostics(&mut normalized);
    normalized
}

fn sort_diagnostics(diagnostics: &mut [Value]) {
    diagnostics.sort_by(|left, right| {
        (
            left["line"].as_u64(),
            left["col"].as_u64(),
            left["code"].as_str(),
            left["message"].as_str(),
        )
            .cmp(&(
                right["line"].as_u64(),
                right["col"].as_u64(),
                right["code"].as_str(),
                right["message"].as_str(),
            ))
    });
}

fn completion_labels(result: &Value) -> Vec<String> {
    let items = result
        .as_array()
        .or_else(|| result.get("items").and_then(Value::as_array))
        .cloned()
        .unwrap_or_default();
    items
        .iter()
        .filter_map(|item| item["label"].as_str().map(str::to_string))
        .collect()
}

fn flatten_symbol_names(symbols: &[Value], names: &mut Vec<String>) {
    for symbol in symbols {
        if let Some(name) = symbol["name"].as_str() {
            names.push(name.to_string());
        }
        if let Some(children) = symbol["children"].as_array() {
            flatten_symbol_names(children, names);
        }
    }
}

fn location_spans(result: &Value) -> Vec<(String, u64, u64)> {
    let locations = match result {
        Value::Array(locations) => locations.clone(),
        Value::Object(_) => vec![result.clone()],
        _ => Vec::new(),
    };
    locations
        .iter()
        .filter_map(|location| {
            let uri = location["uri"].as_str()?;
            let line = location["range"]["start"]["line"].as_u64()? + 1;
            let character = location["range"]["start"]["character"].as_u64()? + 1;
            Some((uri.to_string(), line, character))
        })
        .collect()
}

#[test]
fn lsp_feature_probes_match_the_shared_corpus() {
    runtime().block_on(async {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path();
        let copied = copy_corpus(base);
        let mut server = TestLsp::start(base).await;
        let mark = server.mark();
        server.start_session(Some(&["utf-8"])).await;
        for (path, _) in &copied {
            let text = std::fs::read_to_string(path).expect("corpus text");
            server.open(path, &text, 1).await;
        }
        // The opens are fast but not instantaneous: an early debounced analysis
        // can publish the first file while later files are still being opened.
        // The last file cannot have version 1 before every earlier overlay
        // exists, so waiting for it guarantees one analysis saw the whole
        // corpus instead of racing the per-open debounce.
        let last = copied.last().map(|(path, _)| path.clone()).expect("corpus");
        server.wait_version(&last, 1, mark).await;

        let probes: Vec<Value> = serde_json::from_str(
            &std::fs::read_to_string(corpus_root().join("feature-probes.json")).expect("features"),
        )
        .expect("feature json");
        for probe in probes {
            let op = probe["op"].as_str().expect("op");
            let key = probe["path"].as_str().unwrap_or("");
            let path = base.join(key.replace('/', std::path::MAIN_SEPARATOR_STR));
            let uri = TestLsp::uri(&path);
            let line = probe["line"].as_u64().unwrap_or(1);
            let character = probe["col"].as_u64().unwrap_or(1);
            match op {
                "hover" => {
                    let result = server
                        .request(
                            "textDocument/hover",
                            json!({
                                "textDocument": {"uri": uri},
                                "position": {"line": line - 1, "character": character - 1},
                            }),
                        )
                        .await;
                    let value = result["contents"]["value"]
                        .as_str()
                        .or_else(|| result["contents"].as_str())
                        .unwrap_or("");
                    for needle in probe["contains"].as_array().expect("contains") {
                        let needle = needle.as_str().expect("needle");
                        assert!(
                            value.contains(needle),
                            "{key} hover lacks {needle:?}: {value}"
                        );
                    }
                }
                "definition" => {
                    let result = server
                        .at("textDocument/definition", &path, line - 1, character - 1)
                        .await;
                    let spans = location_spans(&result);
                    let expected = probe["expect"].as_array().expect("expect");
                    assert_eq!(spans.len(), expected.len(), "{spans:?}");
                    for (span, expected) in spans.iter().zip(expected) {
                        let expected_path = base.join(
                            expected["path"]
                                .as_str()
                                .expect("path")
                                .replace('/', std::path::MAIN_SEPARATOR_STR),
                        );
                        assert_eq!(span.0, TestLsp::uri(&expected_path));
                        assert_eq!(span.1, expected["line"].as_u64().expect("line"));
                        assert_eq!(span.2, expected["col"].as_u64().expect("col"));
                    }
                }
                "references" => {
                    let result = server
                        .request(
                            "textDocument/references",
                            json!({
                                "textDocument": {"uri": uri},
                                "position": {"line": line - 1, "character": character - 1},
                                "context": {"includeDeclaration": true},
                            }),
                        )
                        .await;
                    let spans = location_spans(&result);
                    assert!(
                        spans.len() >= probe["min"].as_u64().expect("min") as usize,
                        "{spans:?}"
                    );
                    for expected in probe["expect_contains"].as_array().expect("contains") {
                        let expected_path = base.join(
                            expected["path"]
                                .as_str()
                                .expect("path")
                                .replace('/', std::path::MAIN_SEPARATOR_STR),
                        );
                        let expected_uri = TestLsp::uri(&expected_path);
                        let expected_line = expected["line"].as_u64().expect("line");
                        let expected_col = expected["col"].as_u64().expect("col");
                        assert!(
                            spans.iter().any(|span| {
                                span.0 == expected_uri
                                    && span.1 == expected_line
                                    && span.2 == expected_col
                            }),
                            "references lack {}:{expected_line}:{expected_col}: {spans:?}",
                            expected["path"].as_str().unwrap_or("")
                        );
                    }
                }
                "prepareRename" => {
                    let result = server
                        .at("textDocument/prepareRename", &path, line - 1, character - 1)
                        .await;
                    assert_eq!(
                        result["placeholder"].as_str().unwrap_or(""),
                        probe["placeholder"].as_str().expect("placeholder")
                    );
                }
                "rename" => {
                    let result = server
                        .request(
                            "textDocument/rename",
                            json!({
                                "textDocument": {"uri": uri},
                                "position": {"line": line - 1, "character": character - 1},
                                "newName": probe["new_name"].as_str().expect("new name"),
                            }),
                        )
                        .await;
                    let changes = result["changes"].as_object().expect("changes");
                    for expected in probe["changed_files"].as_array().expect("changed") {
                        let expected = expected.as_str().expect("file");
                        assert!(
                            changes.keys().any(|uri| uri.contains(expected)),
                            "rename lacks {expected}: {:?}",
                            changes.keys().collect::<Vec<_>>()
                        );
                    }
                    let edits = changes
                        .values()
                        .filter_map(Value::as_array)
                        .flatten()
                        .collect::<Vec<_>>();
                    assert_eq!(
                        edits.len(),
                        probe["expected_edits"].as_u64().expect("edits") as usize
                    );
                    assert!(edits.iter().all(|edit| {
                        edit["newText"].as_str()
                            == Some(probe["new_name"].as_str().expect("new name"))
                    }));
                }
                "documentSymbols" => {
                    let result = server
                        .request(
                            "textDocument/documentSymbol",
                            json!({
                                "textDocument": {"uri": uri},
                            }),
                        )
                        .await;
                    let mut names = Vec::new();
                    if let Some(symbols) = result.as_array() {
                        flatten_symbol_names(symbols, &mut names);
                    }
                    for expected in probe["contains"].as_array().expect("contains") {
                        let expected = expected.as_str().expect("name");
                        assert!(names.iter().any(|name| name == expected), "{names:?}");
                    }
                }
                "workspaceSymbols" => {
                    let result = server
                        .request(
                            "workspace/symbol",
                            json!({"query": probe["query"].as_str().expect("query")}),
                        )
                        .await;
                    let names = result
                        .as_array()
                        .map(|symbols| {
                            symbols
                                .iter()
                                .filter_map(|symbol| symbol["name"].as_str())
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    for expected in probe["contains"].as_array().expect("contains") {
                        let expected = expected.as_str().expect("name");
                        assert!(names.contains(&expected), "{names:?}");
                    }
                }
                "folding" => {
                    let result = server
                        .request(
                            "textDocument/foldingRange",
                            json!({
                                "textDocument": {"uri": uri},
                            }),
                        )
                        .await;
                    assert!(
                        result.as_array().map(Vec::len).unwrap_or(0)
                            >= probe["min"].as_u64().expect("min") as usize,
                        "{result}"
                    );
                }
                "semanticTokens" => {
                    let result = server
                        .request(
                            "textDocument/semanticTokens/full",
                            json!({
                                "textDocument": {"uri": uri},
                            }),
                        )
                        .await;
                    let token_count = result["data"].as_array().map(Vec::len).unwrap_or(0) / 5;
                    assert!(
                        token_count >= probe["min"].as_u64().expect("min") as usize,
                        "{token_count} tokens"
                    );
                }
                "signatureHelp" => {
                    let result = server
                        .at("textDocument/signatureHelp", &path, line - 1, character - 1)
                        .await;
                    let needle = probe["label_contains"].as_str().expect("label");
                    assert!(
                        result["signatures"]
                            .as_array()
                            .is_some_and(|signatures| signatures
                                .iter()
                                .any(|signature| signature["label"]
                                    .as_str()
                                    .is_some_and(|label| label.contains(needle)))),
                        "{result}"
                    );
                    assert_eq!(
                        result["activeParameter"].as_u64(),
                        probe["active_parameter"].as_u64()
                    );
                }
                "codeActions" => {
                    let start = &probe["start"];
                    let end = &probe["end"];
                    let result = server
                        .request(
                            "textDocument/codeAction",
                            json!({
                                "textDocument": {"uri": uri},
                                "range": {
                                    "start": {
                                        "line": start["line"].as_u64().expect("line") - 1,
                                        "character": start["col"].as_u64().expect("col") - 1,
                                    },
                                    "end": {
                                        "line": end["line"].as_u64().expect("line") - 1,
                                        "character": end["col"].as_u64().expect("col") - 1,
                                    },
                                },
                                "context": {"diagnostics": []},
                            }),
                        )
                        .await;
                    let titles = result
                        .as_array()
                        .map(|actions| {
                            actions
                                .iter()
                                .filter_map(|action| action["title"].as_str())
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    for expected in probe["contains_titles"].as_array().expect("titles") {
                        let expected = expected.as_str().expect("title");
                        assert!(titles.contains(&expected), "{titles:?}");
                    }
                }
                "inlayHints" => {
                    let result = server
                        .request(
                            "textDocument/inlayHint",
                            json!({
                                "textDocument": {"uri": uri},
                                "range": {
                                    "start": {"line": line - 1, "character": 0},
                                    "end": {"line": line, "character": 0},
                                },
                            }),
                        )
                        .await;
                    let labels = result
                        .as_array()
                        .map(|hints| {
                            hints
                                .iter()
                                .filter_map(|hint| hint["label"].as_str())
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    for expected in probe["contains"].as_array().expect("contains") {
                        let expected = expected.as_str().expect("label");
                        assert!(
                            labels.iter().any(|label| label.contains(expected)),
                            "{labels:?}"
                        );
                    }
                }
                "codeLenses" => {
                    let result = server
                        .request(
                            "textDocument/codeLens",
                            json!({
                                "textDocument": {"uri": uri},
                            }),
                        )
                        .await;
                    let titles = result
                        .as_array()
                        .map(|lenses| {
                            lenses
                                .iter()
                                .filter_map(|lens| lens["command"]["title"].as_str())
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    for expected in probe["contains"].as_array().expect("contains") {
                        let expected = expected.as_str().expect("title");
                        assert!(
                            titles.iter().any(|title| title.contains(expected)),
                            "{titles:?}"
                        );
                    }
                }
                "rangeFormatting" => {
                    let start = &probe["start"];
                    let end = &probe["end"];
                    let result = server
                        .request(
                            "textDocument/rangeFormatting",
                            json!({
                                "textDocument": {"uri": uri},
                                "range": {
                                    "start": {
                                        "line": start["line"].as_u64().expect("line") - 1,
                                        "character": start["col"].as_u64().expect("col") - 1,
                                    },
                                    "end": {
                                        "line": end["line"].as_u64().expect("line") - 1,
                                        "character": end["col"].as_u64().expect("col") - 1,
                                    },
                                },
                                "options": {"tabSize": 4, "insertSpaces": true},
                            }),
                        )
                        .await;
                    let expected = probe["new_text_contains"].as_str().expect("text");
                    assert!(
                        result.as_array().is_some_and(|edits| edits
                            .iter()
                            .any(|edit| edit["newText"]
                                .as_str()
                                .is_some_and(|text| text.contains(expected)))),
                        "{result}"
                    );
                }
                "onTypeFormatting" => {
                    let result = server
                        .request(
                            "textDocument/onTypeFormatting",
                            json!({
                                "textDocument": {"uri": uri},
                                "position": {"line": line - 1, "character": character - 1},
                                "ch": probe["ch"].as_str().expect("ch"),
                                "options": {"tabSize": 4, "insertSpaces": true},
                            }),
                        )
                        .await;
                    let expected = probe["new_text_contains"].as_str().expect("text");
                    assert!(
                        result.as_array().is_some_and(|edits| edits
                            .iter()
                            .any(|edit| edit["newText"]
                                .as_str()
                                .is_some_and(|text| text.contains(expected)))),
                        "{result}"
                    );
                }
                other => panic!("unknown feature probe {other}"),
            }
        }
    });
}

#[test]
fn lsp_matches_the_shared_corpus() {
    runtime().block_on(async {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path();
        let copied = copy_corpus(base);
        let mut server = TestLsp::start(base).await;
        let mark = server.mark();
        server.start_session(Some(&["utf-8"])).await;
        for (path, key) in &copied {
            let text = std::fs::read_to_string(path).expect("corpus text");
            server.open(path, &text, 1).await;
            let _ = key;
        }
        // The opens are fast but not instantaneous: an early debounced analysis
        // can publish the first file while later files are still being opened.
        // The last file cannot have version 1 before every earlier overlay
        // exists, so waiting for it guarantees one analysis saw the whole
        // corpus instead of racing the per-open debounce.
        let last = copied.last().map(|(path, _)| path.clone()).expect("corpus");
        server.wait_version(&last, 1, mark).await;

        let expected: Value = serde_json::from_str(
            &std::fs::read_to_string(corpus_root().join("expected-diagnostics.json"))
                .expect("diagnostics"),
        )
        .expect("diagnostics json");
        let expected = expected.as_object().expect("diagnostics object");
        for (path, key) in &copied {
            let actual =
                normalize_diagnostics(server.latest_diagnostics(path).as_ref(), base, &copied);
            let mut expected_for_file = expected
                .get(key)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            sort_diagnostics(&mut expected_for_file);
            assert_eq!(
                Value::Array(actual),
                Value::Array(expected_for_file),
                "LSP diagnostics differ for {key}"
            );
        }

        for (path, key) in &copied {
            let text = std::fs::read_to_string(path).expect("corpus text");
            let result = server
                .request(
                    "textDocument/formatting",
                    json!({
                        "textDocument": {"uri": TestLsp::uri(path)},
                        "options": {"tabSize": 4, "insertSpaces": true},
                    }),
                )
                .await;
            let formatted = result
                .as_array()
                .and_then(|edits| edits.first())
                .and_then(|edit| edit["newText"].as_str())
                .map_or_else(|| text.clone(), str::to_string);
            let expected = std::fs::read_to_string(corpus_root().join("expected-format").join(key))
                .unwrap_or_else(|_| panic!("missing expected-format/{key}"));
            assert_eq!(formatted, expected, "LSP formatting differs for {key}");
        }

        let probes: Vec<Value> = serde_json::from_str(
            &std::fs::read_to_string(corpus_root().join("completion-probes.json")).expect("probes"),
        )
        .expect("probes json");
        for probe in probes {
            let key = probe["path"].as_str().expect("probe path");
            let path = base.join(key);
            let line = probe["line"].as_u64().expect("line") - 1;
            let character = probe["col"].as_u64().expect("col") - 1;
            let labels = completion_labels(
                &server
                    .at("textDocument/completion", &path, line, character)
                    .await,
            );
            for expected in probe["contains"].as_array().expect("contains") {
                let expected = expected.as_str().expect("label");
                assert!(
                    labels.iter().any(|label| label == expected),
                    "LSP completion {key}:{}:{} lacks {expected:?}; got {labels:?}",
                    line + 1,
                    character + 1
                );
            }
        }
    });
}
