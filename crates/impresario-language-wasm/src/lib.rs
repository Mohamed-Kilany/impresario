//! Raw WebAssembly ABI for the standalone VS Code extension.
//!
//! The extension has no Rust runtime and no `wasm-bindgen`; it allocates a
//! buffer in this module's linear memory, writes UTF-8 JSON, calls
//! [`impresario_invoke`], reads the JSON response and frees both buffers. One
//! `analyze` request installs the workspace; every later request is a feature
//! call on that snapshot, exactly like the embedded LSP backend.

use std::cell::RefCell;
use std::path::PathBuf;

use impresario_core::apps::{self, AppsConfig};
use impresario_core::config::{self, DevicesConfig};
use impresario_core::diag::Severity;
use impresario_core::platforms::Platform;
use impresario_core::validation::DeviceRef;
use impresario_language::diagnostics::DiagnosticInfo;
use impresario_language::model::{AppInfo, SourceFile};
use impresario_language::service::Language;
use impresario_language::text::Position;
use serde::Deserialize;
use serde_json::{json, Value};

thread_local! {
    static LANGUAGE: RefCell<Option<Language>> = const { RefCell::new(None) };
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Request {
    op: String,
    #[serde(default)]
    sources: Vec<SourceInput>,
    #[serde(default)]
    devices: Vec<DeviceInput>,
    devices_yaml: Option<String>,
    devices_path: Option<String>,
    #[serde(default)]
    apps: Vec<AppInput>,
    apps_yaml: Option<String>,
    apps_path: Option<String>,
    root: Option<String>,
    path: Option<String>,
    position: Option<PositionInput>,
    start: Option<PositionInput>,
    end: Option<PositionInput>,
    #[serde(default = "default_true")]
    include_declaration: bool,
    new_name: Option<String>,
    #[serde(default)]
    query: String,
    ch: Option<String>,
    start_line: Option<u32>,
    end_line: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct SourceInput {
    path: String,
    text: String,
}

#[derive(Debug, Deserialize)]
struct DeviceInput {
    name: String,
    platform: String,
}

#[derive(Debug, Deserialize)]
struct AppInput {
    name: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Copy)]
struct PositionInput {
    line: u32,
    col: u32,
}

fn default_true() -> bool {
    true
}

impl PositionInput {
    fn service(self) -> Position {
        Position::new(self.line.max(1), self.col.max(1))
    }
}

/// Run one request on the host (also the unit-test entry point).
fn dispatch(request: &Request) -> Result<Value, String> {
    match request.op.as_str() {
        "analyze" => analyze(request),
        "reset" => {
            LANGUAGE.with(|language| *language.borrow_mut() = None);
            Ok(json!({"ok": true}))
        }
        "completion" => with_language(request, |language, path| {
            let position = request_position(request)?.service();
            Ok(json!(language.completion(&path, position)))
        }),
        "hover" => with_language(request, |language, path| {
            let position = request_position(request)?.service();
            Ok(json!(language.hover(&path, position)))
        }),
        "definition" => with_language(request, |language, path| {
            let position = request_position(request)?.service();
            Ok(json!(language.definition(&path, position)))
        }),
        "references" => with_language(request, |language, path| {
            let position = request_position(request)?.service();
            Ok(json!(language.references(
                &path,
                position,
                request.include_declaration
            )))
        }),
        "prepareRename" => with_language(request, |language, path| {
            let position = request_position(request)?.service();
            Ok(json!(language.prepare_rename(&path, position)))
        }),
        "rename" => with_language(request, |language, path| {
            let position = request_position(request)?.service();
            let new_name = request
                .new_name
                .as_deref()
                .ok_or_else(|| "rename needs `newName`".to_string())?;
            language
                .rename(&path, position, new_name)
                .map(|edit| json!(edit))
        }),
        "documentSymbols" => with_language(request, |language, path| {
            Ok(json!(language.document_symbols(&path)))
        }),
        "workspaceSymbols" => with_language_workspace(request, |language| {
            Ok(json!(language.workspace_symbols(&request.query)))
        }),
        "folding" => with_language(request, |language, path| {
            Ok(json!(language.folding_ranges(&path)))
        }),
        "semanticTokens" => with_language(request, |language, path| {
            Ok(json!(language.semantic_tokens(&path)))
        }),
        "signatureHelp" => with_language(request, |language, path| {
            let position = request_position(request)?.service();
            Ok(json!(language.signature_help(&path, position)))
        }),
        "codeActions" => with_language(request, |language, path| {
            let start = request_start(request)?.service();
            let end = request_end(request)?.service();
            Ok(json!(language.code_actions(&path, start, end)))
        }),
        "format" => with_language(request, |language, path| {
            language.format_document(&path).map(|edit| json!(edit))
        }),
        "rangeFormatting" => with_language(request, |language, path| {
            let start = request_start(request)?.service();
            let end = request_end(request)?.service();
            language
                .format_range(&path, start, end)
                .map(|edits| json!(edits))
        }),
        "onTypeFormatting" => with_language(request, |language, path| {
            let position = request_position(request)?.service();
            let ch = request
                .ch
                .as_deref()
                .and_then(|ch| ch.chars().next())
                .ok_or_else(|| "onTypeFormatting needs one character in `ch`".to_string())?;
            language
                .format_on_type(&path, position, ch)
                .map(|edits| json!(edits))
        }),
        "inlayHints" => with_language(request, |language, path| {
            let range = match (request.start_line, request.end_line) {
                (Some(start), Some(end)) => Some((start.max(1), end.max(start.max(1)))),
                _ => None,
            };
            Ok(json!(language.inlay_hints(&path, range)))
        }),
        "codeLenses" => with_language(request, |language, path| {
            Ok(json!(language.code_lenses(&path)))
        }),
        "diagnostics" => with_language(request, |language, path| {
            Ok(json!(language.diagnostics_for(&path)))
        }),
        other => Err(format!("unknown operation '{other}'")),
    }
}

fn request_position(request: &Request) -> Result<PositionInput, String> {
    request
        .position
        .ok_or_else(|| "missing `position`".to_string())
}

fn request_start(request: &Request) -> Result<PositionInput, String> {
    request
        .start
        .or(request.position)
        .ok_or_else(|| "missing `start` or `position`".to_string())
}

fn request_end(request: &Request) -> Result<PositionInput, String> {
    request
        .end
        .or(request.position)
        .ok_or_else(|| "missing `end` or `position`".to_string())
}

fn with_language(
    request: &Request,
    feature: impl FnOnce(&Language, PathBuf) -> Result<Value, String>,
) -> Result<Value, String> {
    let path = request
        .path
        .as_deref()
        .map(PathBuf::from)
        .ok_or_else(|| "missing `path`".to_string())?;
    with_language_workspace(request, |language| feature(language, path))
}

fn with_language_workspace(
    _request: &Request,
    feature: impl FnOnce(&Language) -> Result<Value, String>,
) -> Result<Value, String> {
    LANGUAGE.with(|language| {
        let language = language.borrow();
        let language = language
            .as_ref()
            .ok_or_else(|| "call `analyze` before feature requests".to_string())?;
        feature(language).map(|result| json!({"ok": true, "result": result}))
    })
}

fn analyze(request: &Request) -> Result<Value, String> {
    let mut diagnostics = std::collections::BTreeMap::new();
    let devices = match (&request.devices_yaml, request.devices_path.as_deref()) {
        (Some(source), path) => {
            let path = PathBuf::from(path.unwrap_or("impresario.devices.config"));
            let outcome = config::parse_devices(source, &path);
            collect_core(&mut diagnostics, &outcome.diagnostics, &path);
            outcome
                .config
                .map(|config| device_refs(&config))
                .unwrap_or_default()
        }
        _ => request
            .devices
            .iter()
            .filter_map(|device| {
                device
                    .platform
                    .parse::<Platform>()
                    .ok()
                    .map(|platform| DeviceRef::new(device.name.clone(), platform))
            })
            .collect(),
    };
    let (apps, check_apps) = match (&request.apps_yaml, request.apps_path.as_deref()) {
        (Some(source), path) => {
            let path = PathBuf::from(path.unwrap_or("impresario.apps.config"));
            let outcome = apps::parse_apps(source, &path);
            collect_core(&mut diagnostics, &outcome.diagnostics, &path);
            let authoritative = outcome.config.is_some();
            let infos = outcome.config.as_ref().map(app_infos).unwrap_or_default();
            (infos, authoritative)
        }
        _ => (
            request
                .apps
                .iter()
                .map(|app| AppInfo {
                    name: app.name.clone(),
                    description: app.description.clone(),
                })
                .collect::<Vec<_>>(),
            !request.apps.is_empty(),
        ),
    };
    let sources = request
        .sources
        .iter()
        .map(|source| SourceFile {
            path: PathBuf::from(&source.path),
            text: source.text.clone(),
        })
        .collect::<Vec<_>>();
    let root = request.root.clone().map(PathBuf::from);
    let language = if check_apps {
        Language::analyze_with_apps(&sources, &devices, &apps, root)
    } else {
        Language::analyze(&sources, &devices, &apps, root)
    };
    for (path, infos) in &language.diagnostics {
        diagnostics
            .entry(path.clone())
            .or_insert_with(Vec::new)
            .extend(infos.iter().cloned());
    }
    for infos in diagnostics.values_mut() {
        infos.sort_by(|left, right| {
            (
                left.span.file.clone(),
                left.span.line,
                left.span.col,
                &left.code,
            )
                .cmp(&(
                    right.span.file.clone(),
                    right.span.line,
                    right.span.col,
                    &right.code,
                ))
        });
        infos.dedup_by(|left, right| {
            left.code == right.code && left.span == right.span && left.message == right.message
        });
    }
    LANGUAGE.with(|slot| *slot.borrow_mut() = Some(language));
    Ok(json!({
        "ok": true,
        "result": {
            "diagnostics": diagnostics,
            "devices": devices.iter().map(|device| json!({
                "name": device.name,
                "platform": device.platform.as_str(),
            })).collect::<Vec<_>>(),
            "apps": apps.iter().map(|app| app.name.clone()).collect::<Vec<_>>(),
        }
    }))
}

fn device_refs(config: &DevicesConfig) -> Vec<DeviceRef> {
    config
        .devices
        .iter()
        .map(|device| DeviceRef::new(device.name.clone(), device.platform))
        .collect()
}

fn app_infos(config: &AppsConfig) -> Vec<AppInfo> {
    config
        .apps
        .iter()
        .map(|app| AppInfo {
            name: app.name.clone(),
            description: app.description.clone(),
        })
        .collect()
}

fn collect_core(
    by_file: &mut std::collections::BTreeMap<PathBuf, Vec<DiagnosticInfo>>,
    diagnostics: &impresario_core::diag::Diagnostics,
    fallback: &std::path::Path,
) {
    for diagnostic in diagnostics {
        let span = diagnostic
            .span
            .clone()
            .unwrap_or_else(|| impresario_core::span::Span::new(fallback, 1, 1, 0));
        let info = DiagnosticInfo {
            code: diagnostic.code.as_str().to_string(),
            severity: match diagnostic.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
            }
            .to_string(),
            message: diagnostic.message.clone(),
            help: diagnostic.help.clone(),
            span,
            related: Vec::new(),
        };
        by_file
            .entry(info.span.file.clone())
            .or_default()
            .push(info);
    }
}

#[no_mangle]
pub extern "C" fn impresario_alloc(len: usize) -> *mut u8 {
    let mut buffer = vec![0u8; len];
    let pointer = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    pointer
}

/// # Safety
/// `pointer` must come from [`impresario_alloc`] with the same `len`.
#[no_mangle]
pub unsafe extern "C" fn impresario_free(pointer: *mut u8, len: usize) {
    if pointer.is_null() || len == 0 {
        return;
    }
    drop(Vec::from_raw_parts(pointer, len, len));
}

/// # Safety
/// `pointer` must point to `len` initialized bytes owned by the caller.
#[no_mangle]
pub unsafe extern "C" fn impresario_invoke(pointer: *const u8, len: usize) -> u64 {
    let input = if pointer.is_null() || len == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(pointer, len)
    };
    let response = std::panic::catch_unwind(|| invoke_bytes(input)).unwrap_or_else(|_| {
        br#"{"ok":false,"error":"panic in impresario language service"}"#.to_vec()
    });
    let mut output = response.into_boxed_slice();
    let length = output.len();
    let pointer = output.as_mut_ptr();
    std::mem::forget(output);
    (u64::from(u32::try_from(length).unwrap_or(u32::MAX)) << 32)
        | u64::from(u32::try_from(pointer as usize).unwrap_or(0))
}

fn invoke_bytes(input: &[u8]) -> Vec<u8> {
    let response = match serde_json::from_slice::<Request>(input) {
        Ok(request) => match dispatch(&request) {
            Ok(mut value) => {
                if let Some(object) = value.as_object_mut() {
                    object.insert("ok".to_string(), json!(true));
                }
                value
            }
            Err(error) => json!({"ok": false, "error": error}),
        },
        Err(error) => json!({"ok": false, "error": format!("invalid JSON request: {error}")}),
    };
    serde_json::to_vec(&response)
        .unwrap_or_else(|_| br#"{"ok":false,"error":"cannot serialize the response"}"#.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(value: Value) -> Value {
        let request = serde_json::from_value::<Request>(value).expect("request");
        dispatch(&request).expect("dispatch")
    }

    #[test]
    fn analyze_then_feature_requests_share_one_service() {
        let analyzed = request(json!({
            "op": "analyze",
            "sources": [
                {
                    "path": "scenes/LoginScreen.scene",
                    "text": "Scene: LoginScreen\nPlatforms: cross-platform\n\n| Piece | web |\n|---|---|\n| LoginButton | #login |\n"
                },
                {
                    "path": "plays/login.play",
                    "text": "Play: Login\n\nAct: A\n    Perform: Missing\n"
                }
            ],
            "devices": [{"name": "web1", "platform": "web"}]
        }));
        let diagnostics = analyzed["result"]["diagnostics"]
            .as_object()
            .expect("diagnostics");
        assert!(
            diagnostics["plays/login.play"]
                .as_array()
                .expect("play diagnostics")
                .iter()
                .any(|diagnostic| diagnostic["code"] == json!("IMP-DSL-031")),
            "{analyzed}"
        );

        let symbols = request(json!({
            "op": "workspaceSymbols",
            "query": "Login"
        }));
        assert!(
            symbols["result"]
                .as_array()
                .expect("symbols")
                .iter()
                .any(|symbol| symbol["name"] == json!("LoginScreen")),
            "{symbols}"
        );

        let formatted = request(json!({
            "op": "format",
            "path": "scenes/LoginScreen.scene"
        }));
        assert!(
            formatted["result"]["new_text"]
                .as_str()
                .unwrap_or("")
                .contains("| LoginButton |"),
            "{formatted}"
        );
    }

    #[test]
    fn unknown_operations_and_missing_state_are_errors() {
        let request = serde_json::from_value::<Request>(json!({"op": "nope"})).expect("request");
        assert!(dispatch(&request).is_err());
        LANGUAGE.with(|language| *language.borrow_mut() = None);
        let request = serde_json::from_value::<Request>(json!({
            "op": "hover",
            "path": "a.scene",
            "position": {"line": 1, "col": 1}
        }))
        .expect("request");
        assert!(dispatch(&request).is_err());
    }
}
