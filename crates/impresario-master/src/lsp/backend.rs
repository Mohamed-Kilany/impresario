//! The language-server backend: state, the debounced analysis worker and the
//! `LanguageServer` implementation (spec section 13).
//!
//! Every feature is routed through `impresario-language`, the same service the
//! standalone VS Code extension compiles to WASM, so the two editor paths
//! cannot drift apart.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard, Weak};
use std::time::Duration;

use impresario_language::model::SourceFile as EditorSourceFile;
use impresario_language::service::Language as EditorLanguage;
use impresario_language::text::Position as ServicePosition;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tower_lsp_server::jsonrpc::{Error as RpcError, Result as RpcResult};
use tower_lsp_server::ls_types::{
    CodeActionKind, CodeActionOptions, CodeActionOrCommand, CodeActionParams,
    CodeActionProviderCapability, CodeActionResponse, CodeLensOptions, CodeLensParams,
    CompletionOptions, CompletionParams, CompletionResponse, DidChangeConfigurationParams,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, DocumentFormattingParams, DocumentOnTypeFormattingOptions,
    DocumentOnTypeFormattingParams, DocumentRangeFormattingParams, DocumentSymbolParams,
    DocumentSymbolResponse, FoldingRangeParams, FoldingRangeProviderCapability,
    GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverParams, HoverProviderCapability,
    InitializeParams, InitializeResult, InitializedParams, InlayHint, InlayHintOptions,
    InlayHintParams, InlayHintServerCapabilities, Location, OneOf, Position, PrepareRenameResponse,
    ReferenceParams, RenameOptions, RenameParams, SemanticTokensFullOptions, SemanticTokensLegend,
    SemanticTokensOptions, SemanticTokensParams, SemanticTokensRangeParams,
    SemanticTokensRangeResult, SemanticTokensResult, SemanticTokensServerCapabilities,
    ServerCapabilities, ServerInfo, SignatureHelp, SignatureHelpOptions, SignatureHelpParams,
    TextDocumentPositionParams, TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit, Uri,
    WorkDoneProgressOptions, WorkspaceEdit, WorkspaceSymbolParams, WorkspaceSymbolResponse,
};
use tower_lsp_server::{Client, LanguageServer};

use super::analyze::{self, Analysis, Overlay};
use super::convert;
use super::position::{self, Encoding};
use super::watch::{specs_for, Watches};

/// The debounce of diagnostics after `didOpen`/`didChange`/`didSave` and
/// after a file-watch burst (spec section 13).
const DIAGNOSTIC_DEBOUNCE: Duration = Duration::from_millis(150);

/// Paths the server was started for (`--workspace`, `--config`,
/// `--devices-config`).
#[derive(Debug, Clone)]
pub struct Ctx {
    pub base: PathBuf,
    pub config_path: PathBuf,
    pub devices_path: PathBuf,
}

/// The LSP backend handed to `tower-lsp-server`.
pub struct Backend {
    inner: Arc<Inner>,
}

struct Inner {
    client: Client,
    ctx: Ctx,
    /// Overridable from the `impresario.config` / `impresario.devices`
    /// workspace settings.
    config_path: RwLock<PathBuf>,
    devices_path: RwLock<PathBuf>,
    encoding: RwLock<Encoding>,
    /// Unsaved editor buffers, full sync (spec section 13).
    overlays: Mutex<BTreeMap<PathBuf, Overlay>>,
    /// The text of the last analysis, used as a fallback for files that are
    /// neither open nor reachable through the service.
    sources: Mutex<BTreeMap<PathBuf, String>>,
    /// The shared editor service built by the last analysis.
    language: Mutex<Option<EditorLanguage>>,
    last_published: Mutex<BTreeSet<PathBuf>>,
    watches: Mutex<Option<Watches>>,
    wake: mpsc::UnboundedSender<()>,
    pending: Mutex<Option<mpsc::UnboundedReceiver<()>>>,
    cancel: CancellationToken,
    worker_started: AtomicBool,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn read<T>(rw: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    rw.read().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write<T>(rw: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    rw.write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn path_of(uri: &Uri) -> Option<PathBuf> {
    uri.to_file_path().map(|path| path.into_owned())
}

impl Backend {
    /// Build the backend for one workspace. Blocks on nothing: the first
    /// analysis runs after `initialized`, debounced like every later one.
    #[must_use]
    pub fn new(client: Client, ctx: Ctx) -> Self {
        let (wake, pending) = mpsc::unbounded_channel();
        let config_path = ctx.config_path.clone();
        let devices_path = ctx.devices_path.clone();
        Self {
            inner: Arc::new(Inner {
                client,
                ctx,
                config_path: RwLock::new(config_path),
                devices_path: RwLock::new(devices_path),
                encoding: RwLock::new(Encoding::default()),
                overlays: Mutex::new(BTreeMap::new()),
                sources: Mutex::new(BTreeMap::new()),
                language: Mutex::new(None),
                last_published: Mutex::new(BTreeSet::new()),
                watches: Mutex::new(None),
                wake,
                pending: Mutex::new(Some(pending)),
                cancel: CancellationToken::new(),
                worker_started: AtomicBool::new(false),
            }),
        }
    }
}

impl Inner {
    fn enqueue(&self) {
        let _ = self.wake.send(());
    }

    /// Spawn the debounce + analysis worker exactly once.
    ///
    /// The worker holds the backend only weakly, so it stops when the server
    /// (and with it the last real owner) goes away.
    fn start_worker(self: &Arc<Self>) {
        if self.worker_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let Some(receiver) = lock(&self.pending).take() else {
            return;
        };
        let weak = Arc::downgrade(self);
        let cancel = self.cancel.clone();
        tokio::spawn(async move { worker(weak, cancel, receiver).await });
    }

    fn encoding(&self) -> Encoding {
        *read(&self.encoding)
    }

    /// Overlay content first, then the last analysis, then disk.
    fn document_text(&self, path: &Path) -> Option<String> {
        if let Some(overlay) = lock(&self.overlays).get(path) {
            return Some(overlay.text.clone());
        }
        if let Some(source) = lock(&self.sources).get(path) {
            return Some(source.clone());
        }
        std::fs::read_to_string(path).ok()
    }

    /// The last analysed service, if any.
    fn language(&self) -> Option<EditorLanguage> {
        lock(&self.language).clone()
    }

    /// The service with the current buffer of `path` applied. Before the first
    /// analysis (or for a brand-new file) a one-file service is enough for
    /// syntax-level features such as completion and formatting.
    fn language_for(&self, path: &Path) -> Option<EditorLanguage> {
        let text = self.document_text(path)?;
        let mut language = self.language().unwrap_or_else(|| {
            EditorLanguage::analyze(
                &[EditorSourceFile {
                    path: path.to_path_buf(),
                    text: text.clone(),
                }],
                &[],
                &[],
                Some(self.ctx.base.clone()),
            )
        });
        language.sources.insert(path.to_path_buf(), text);
        Some(language)
    }

    fn snapshot(
        &self,
    ) -> (
        PathBuf,
        PathBuf,
        PathBuf,
        Encoding,
        BTreeMap<PathBuf, Overlay>,
    ) {
        (
            self.ctx.base.clone(),
            read(&self.config_path).clone(),
            read(&self.devices_path).clone(),
            *read(&self.encoding),
            lock(&self.overlays).clone(),
        )
    }

    /// One full analysis pass on a blocking thread, then update state and
    /// publish diagnostics.
    async fn rebuild(self: &Arc<Self>) {
        let (base, config_path, devices_path, encoding, overlays) = self.snapshot();
        let analyze_config = config_path.clone();
        let analyze_devices = devices_path.clone();
        let result = tokio::task::spawn_blocking(move || {
            analyze::analyze(
                &base,
                &analyze_config,
                &analyze_devices,
                &overlays,
                encoding,
            )
        })
        .await;
        let Ok(analysis) = result else {
            tracing::warn!("lsp: the analysis task failed");
            return;
        };
        self.apply(&analysis, &config_path, &devices_path);
        self.publish(&analysis).await;
    }

    fn apply(&self, analysis: &Analysis, config_path: &Path, devices_path: &Path) {
        *lock(&self.language) = Some(analysis.language.clone());
        *lock(&self.sources) = analysis.sources.clone();

        let desired = specs_for(
            &analysis.scenes_dir,
            &analysis.theatres_dir,
            &analysis.plays_dir,
            config_path,
            devices_path,
            &analysis.apps_path,
        );
        let mut watches = lock(&self.watches);
        if watches.is_none() {
            let wake = self.wake.clone();
            match Watches::new(move || {
                let _ = wake.send(());
            }) {
                Ok(watcher) => *watches = Some(watcher),
                Err(err) => tracing::warn!("lsp: cannot watch the workspace: {err}"),
            }
        }
        if let Some(watcher) = watches.as_mut() {
            watcher.sync(&desired);
        }
    }

    /// Publish diagnostics for every file that has them, every open buffer
    /// (clearing stale ones) and every file that was published before.
    async fn publish(&self, analysis: &Analysis) {
        let versions: BTreeMap<PathBuf, i32> = lock(&self.overlays)
            .iter()
            .map(|(path, overlay)| (path.clone(), overlay.version))
            .collect();
        let mut paths: BTreeSet<PathBuf> = analysis.by_file.keys().cloned().collect();
        paths.extend(versions.keys().cloned());
        paths.extend(lock(&self.last_published).iter().cloned());
        for path in &paths {
            let diagnostics = analysis.by_file.get(path).cloned().unwrap_or_default();
            let Some(uri) = Uri::from_file_path(path) else {
                continue;
            };
            let version = versions.get(path).copied();
            self.client
                .publish_diagnostics(uri, diagnostics, version)
                .await;
        }
        *lock(&self.last_published) = analysis.by_file.keys().cloned().collect();
    }

    /// `impresario.devices` (and `impresario.config`) from
    /// `didChangeConfiguration` settings or `initializationOptions`.
    ///
    /// Accepted shapes: `{ "impresario": { "devices": "..." } }`,
    /// `{ "impresario.devices": "..." }` and `{ "devices": "..." }` (the keys
    /// are documented in `docs/LSP.md`).
    fn apply_settings(&self, settings: &Value) {
        let nested = settings.get("impresario");
        let pick = |key: &str| -> Option<&str> {
            nested
                .and_then(|scope| scope.get(key))
                .and_then(Value::as_str)
                .or_else(|| {
                    settings
                        .get(format!("impresario.{key}"))
                        .and_then(Value::as_str)
                })
                .or_else(|| settings.get(key).and_then(Value::as_str))
        };
        if let Some(path) = pick("config") {
            *write(&self.config_path) = analyze::resolve(&self.ctx.base, PathBuf::from(path));
        }
        if let Some(path) = pick("devices") {
            *write(&self.devices_path) = analyze::resolve(&self.ctx.base, PathBuf::from(path));
        }
    }

    fn service_position(
        language: &EditorLanguage,
        path: &Path,
        encoding: Encoding,
        position: Position,
    ) -> Option<ServicePosition> {
        let text = language.source(path)?;
        convert::position(text, encoding, position)
    }

    fn hover_of(&self, uri: &Uri, position: Position) -> Option<Hover> {
        let path = path_of(uri)?;
        let language = self.language_for(&path)?;
        let encoding = self.encoding();
        let service_position = Self::service_position(&language, &path, encoding, position)?;
        let hover = language.hover(&path, service_position)?;
        let text = language.source(&path)?;
        Some(convert::hover(text, encoding, hover))
    }

    fn completion_of(&self, uri: &Uri, position: Position) -> Option<CompletionResponse> {
        let path = path_of(uri)?;
        let language = self.language_for(&path)?;
        let encoding = self.encoding();
        let service_position = Self::service_position(&language, &path, encoding, position)?;
        let items = language
            .completion(&path, service_position)
            .into_iter()
            .map(|item| convert::completion_item(&language.sources, encoding, item))
            .collect::<Vec<_>>();
        Some(CompletionResponse::Array(items))
    }

    fn definition_of(&self, uri: &Uri, position: Position) -> Option<GotoDefinitionResponse> {
        let path = path_of(uri)?;
        let language = self.language_for(&path)?;
        let encoding = self.encoding();
        let service_position = Self::service_position(&language, &path, encoding, position)?;
        let mut locations = language
            .definition(&path, service_position)
            .iter()
            .filter_map(|span| convert::location(&language.sources, encoding, span))
            .collect::<Vec<_>>();
        match locations.len() {
            0 => None,
            1 => locations.pop().map(GotoDefinitionResponse::Scalar),
            _ => Some(GotoDefinitionResponse::Array(locations)),
        }
    }

    fn references_of(
        &self,
        uri: &Uri,
        position: Position,
        include_declaration: bool,
    ) -> Option<Vec<Location>> {
        let path = path_of(uri)?;
        let language = self.language_for(&path)?;
        let encoding = self.encoding();
        let service_position = Self::service_position(&language, &path, encoding, position)?;
        Some(
            language
                .references(&path, service_position, include_declaration)
                .iter()
                .filter_map(|span| convert::location(&language.sources, encoding, span))
                .collect(),
        )
    }

    fn prepare_rename_of(&self, uri: &Uri, position: Position) -> Option<PrepareRenameResponse> {
        let path = path_of(uri)?;
        let language = self.language_for(&path)?;
        let encoding = self.encoding();
        let service_position = Self::service_position(&language, &path, encoding, position)?;
        let prepared = language.prepare_rename(&path, service_position)?;
        let text = language.source(&path)?;
        Some(PrepareRenameResponse::RangeWithPlaceholder {
            range: convert::range(text, encoding, &prepared.span),
            placeholder: prepared.placeholder,
        })
    }

    fn rename_of(
        &self,
        uri: &Uri,
        position: Position,
        new_name: &str,
    ) -> Result<Option<WorkspaceEdit>, String> {
        let path = path_of(uri).ok_or_else(|| "invalid document URI".to_string())?;
        let language = self
            .language_for(&path)
            .ok_or_else(|| format!("unknown document {}", path.display()))?;
        let encoding = self.encoding();
        let service_position = Self::service_position(&language, &path, encoding, position)
            .ok_or_else(|| "position is outside the document".to_string())?;
        let edit = language.rename(&path, service_position, new_name)?;
        convert::workspace_edit(&language.sources, encoding, &edit)
            .map(Some)
            .ok_or_else(|| "cannot convert the workspace edit".to_string())
    }

    fn symbols_of(&self, uri: &Uri) -> Option<DocumentSymbolResponse> {
        let path = path_of(uri)?;
        let language = self.language_for(&path)?;
        let encoding = self.encoding();
        Some(DocumentSymbolResponse::Nested(
            language
                .document_symbols(&path)
                .iter()
                .filter_map(|symbol| convert::document_symbol(&language.sources, encoding, symbol))
                .collect(),
        ))
    }

    fn workspace_symbols_of(&self, query: &str) -> Option<WorkspaceSymbolResponse> {
        let language = self.language()?;
        let encoding = self.encoding();
        Some(WorkspaceSymbolResponse::Nested(
            language
                .workspace_symbols(query)
                .iter()
                .filter_map(|symbol| convert::workspace_symbol(&language.sources, encoding, symbol))
                .collect(),
        ))
    }

    fn folding_of(&self, uri: &Uri) -> Option<Vec<tower_lsp_server::ls_types::FoldingRange>> {
        let path = path_of(uri)?;
        let language = self.language_for(&path)?;
        Some(
            language
                .folding_ranges(&path)
                .iter()
                .map(convert::folding)
                .collect(),
        )
    }

    fn semantic_tokens_full_of(&self, uri: &Uri) -> Option<SemanticTokensResult> {
        let path = path_of(uri)?;
        let language = self.language_for(&path)?;
        let encoding = self.encoding();
        Some(SemanticTokensResult::Tokens(convert::semantic_tokens(
            &language.sources,
            encoding,
            &language.semantic_tokens(&path),
            None,
        )))
    }

    fn semantic_tokens_range_of(
        &self,
        uri: &Uri,
        range: tower_lsp_server::ls_types::Range,
    ) -> Option<SemanticTokensRangeResult> {
        let path = path_of(uri)?;
        let language = self.language_for(&path)?;
        let encoding = self.encoding();
        Some(SemanticTokensRangeResult::Tokens(convert::semantic_tokens(
            &language.sources,
            encoding,
            &language.semantic_tokens(&path),
            Some(range),
        )))
    }

    fn signature_help_of(&self, uri: &Uri, position: Position) -> Option<SignatureHelp> {
        let path = path_of(uri)?;
        let language = self.language_for(&path)?;
        let encoding = self.encoding();
        let service_position = Self::service_position(&language, &path, encoding, position)?;
        let help = language.signature_help(&path, service_position)?;
        Some(convert::signature_help(&help))
    }

    fn code_actions_of(
        &self,
        uri: &Uri,
        range: tower_lsp_server::ls_types::Range,
    ) -> Option<CodeActionResponse> {
        let path = path_of(uri)?;
        let language = self.language_for(&path)?;
        let encoding = self.encoding();
        let text = language.source(&path)?;
        let start = convert::position(text, encoding, range.start)?;
        let end = convert::position(text, encoding, range.end)?;
        Some(
            language
                .code_actions(&path, start, end)
                .iter()
                .filter_map(|action| convert::code_action(&language.sources, encoding, action))
                .map(CodeActionOrCommand::CodeAction)
                .collect(),
        )
    }

    fn formatting_of(&self, uri: &Uri) -> Option<Vec<TextEdit>> {
        let path = path_of(uri)?;
        let language = self.language_for(&path)?;
        let encoding = self.encoding();
        match language.format_document(&path) {
            Ok(edit) => Some(
                edit.iter()
                    .filter_map(|edit| convert::text_edit(&language.sources, encoding, edit))
                    .collect(),
            ),
            Err(err) => {
                tracing::debug!("lsp: cannot format {}: {err}", path.display());
                None
            }
        }
    }

    fn range_formatting_of(
        &self,
        uri: &Uri,
        range: tower_lsp_server::ls_types::Range,
    ) -> Option<Vec<TextEdit>> {
        let path = path_of(uri)?;
        let language = self.language_for(&path)?;
        let encoding = self.encoding();
        let text = language.source(&path)?;
        let start = convert::position(text, encoding, range.start)?;
        let end = convert::position(text, encoding, range.end)?;
        match language.format_range(&path, start, end) {
            Ok(edits) => Some(
                edits
                    .iter()
                    .filter_map(|edit| convert::text_edit(&language.sources, encoding, edit))
                    .collect(),
            ),
            Err(err) => {
                tracing::debug!("lsp: cannot format range of {}: {err}", path.display());
                None
            }
        }
    }

    fn on_type_formatting_of(
        &self,
        uri: &Uri,
        position: Position,
        ch: &str,
    ) -> Option<Vec<TextEdit>> {
        let path = path_of(uri)?;
        let language = self.language_for(&path)?;
        let encoding = self.encoding();
        let service_position = Self::service_position(&language, &path, encoding, position)?;
        let typed = ch.chars().next()?;
        match language.format_on_type(&path, service_position, typed) {
            Ok(edits) => Some(
                edits
                    .iter()
                    .filter_map(|edit| convert::text_edit(&language.sources, encoding, edit))
                    .collect(),
            ),
            Err(err) => {
                tracing::debug!("lsp: cannot format on type in {}: {err}", path.display());
                None
            }
        }
    }

    fn inlay_hints_of(
        &self,
        uri: &Uri,
        range: tower_lsp_server::ls_types::Range,
    ) -> Option<Vec<InlayHint>> {
        let path = path_of(uri)?;
        let language = self.language_for(&path)?;
        let encoding = self.encoding();
        let lines = Some((
            range.start.line.saturating_add(1),
            range.end.line.saturating_add(1),
        ));
        Some(
            language
                .inlay_hints(&path, lines)
                .iter()
                .filter_map(|hint| convert::inlay_hint(&language.sources, encoding, hint))
                .collect(),
        )
    }

    fn code_lenses_of(&self, uri: &Uri) -> Option<Vec<tower_lsp_server::ls_types::CodeLens>> {
        let path = path_of(uri)?;
        let language = self.language_for(&path)?;
        let encoding = self.encoding();
        Some(
            language
                .code_lenses(&path)
                .iter()
                .filter_map(|lens| convert::code_lens(&language.sources, encoding, lens))
                .collect(),
        )
    }
}

/// The debounced analysis worker: wait for a first signal, let the burst
/// settle for [`DIAGNOSTIC_DEBOUNCE`], then analyse and publish.
async fn worker(
    inner: Weak<Inner>,
    cancel: CancellationToken,
    mut receiver: mpsc::UnboundedReceiver<()>,
) {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            message = receiver.recv() => {
                if message.is_none() {
                    break;
                }
            }
        }
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return,
                () = tokio::time::sleep(DIAGNOSTIC_DEBOUNCE) => break,
                message = receiver.recv() => {
                    if message.is_none() {
                        return;
                    }
                }
            }
        }
        while receiver.try_recv().is_ok() {}
        let Some(inner) = inner.upgrade() else {
            return;
        };
        inner.rebuild().await;
    }
}

impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> RpcResult<InitializeResult> {
        let offered = params
            .capabilities
            .general
            .as_ref()
            .and_then(|general| general.position_encodings.clone())
            .unwrap_or_default();
        let encoding = position::negotiate(&offered);
        *write(&self.inner.encoding) = encoding;
        if let Some(options) = &params.initialization_options {
            self.inner.apply_settings(options);
        }
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                position_encoding: Some(encoding.kind()),
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(
                        [":", " ", "'", ">"]
                            .iter()
                            .map(|trigger| (*trigger).to_string())
                            .collect(),
                    ),
                    ..CompletionOptions::default()
                }),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            work_done_progress_options: WorkDoneProgressOptions::default(),
                            legend: SemanticTokensLegend {
                                token_types: convert::TOKEN_TYPES
                                    .iter()
                                    .map(|kind| (*kind).into())
                                    .collect(),
                                token_modifiers: convert::TOKEN_MODIFIERS
                                    .iter()
                                    .map(|modifier| (*modifier).into())
                                    .collect(),
                            },
                            range: Some(true),
                            full: Some(SemanticTokensFullOptions::Bool(true)),
                        },
                    ),
                ),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
                    retrigger_characters: None,
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                }),
                code_action_provider: Some(CodeActionProviderCapability::Options(
                    CodeActionOptions {
                        code_action_kinds: Some(vec![
                            CodeActionKind::QUICKFIX,
                            CodeActionKind::REFACTOR,
                            CodeActionKind::REFACTOR_EXTRACT,
                        ]),
                        work_done_progress_options: WorkDoneProgressOptions::default(),
                        resolve_provider: Some(false),
                    },
                )),
                code_lens_provider: Some(CodeLensOptions {
                    resolve_provider: Some(false),
                }),
                document_formatting_provider: Some(OneOf::Left(true)),
                document_range_formatting_provider: Some(OneOf::Left(true)),
                document_on_type_formatting_provider: Some(DocumentOnTypeFormattingOptions {
                    first_trigger_character: "|".to_string(),
                    more_trigger_character: Some(vec!["\n".to_string()]),
                }),
                rename_provider: Some(OneOf::Right(RenameOptions {
                    prepare_provider: Some(true),
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                })),
                inlay_hint_provider: Some(OneOf::Right(InlayHintServerCapabilities::Options(
                    InlayHintOptions {
                        work_done_progress_options: WorkDoneProgressOptions::default(),
                        resolve_provider: Some(false),
                    },
                ))),
                ..ServerCapabilities::default()
            },
            server_info: Some(ServerInfo {
                name: "impresario".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            offset_encoding: None,
        })
    }

    async fn initialized(&self, _params: InitializedParams) {
        self.inner.start_worker();
        self.inner.enqueue();
    }

    async fn shutdown(&self) -> RpcResult<()> {
        self.inner.cancel.cancel();
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let Some(path) = path_of(&params.text_document.uri) else {
            return;
        };
        lock(&self.inner.overlays).insert(
            path,
            Overlay {
                text: params.text_document.text,
                version: params.text_document.version,
            },
        );
        self.inner.enqueue();
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let Some(path) = path_of(&params.text_document.uri) else {
            return;
        };
        let version = params.text_document.version;
        let text = params
            .content_changes
            .last()
            .map(|change| change.text.clone());
        let mut overlays = lock(&self.inner.overlays);
        match text {
            Some(text) => {
                overlays.insert(path, Overlay { text, version });
            }
            None => {
                if let Some(overlay) = overlays.get_mut(&path) {
                    overlay.version = version;
                }
            }
        }
        drop(overlays);
        self.inner.enqueue();
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let Some(path) = path_of(&params.text_document.uri) else {
            return;
        };
        if let Some(text) = params.text {
            let mut overlays = lock(&self.inner.overlays);
            let version = overlays.get(&path).map_or(0, |overlay| overlay.version);
            overlays.insert(path, Overlay { text, version });
        }
        self.inner.enqueue();
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let Some(path) = path_of(&params.text_document.uri) else {
            return;
        };
        lock(&self.inner.overlays).remove(&path);
        self.inner.enqueue();
    }

    async fn did_change_configuration(&self, params: DidChangeConfigurationParams) {
        self.inner.apply_settings(&params.settings);
        self.inner.enqueue();
    }

    async fn hover(&self, params: HoverParams) -> RpcResult<Option<Hover>> {
        let position = params.text_document_position_params.position;
        let uri = params.text_document_position_params.text_document.uri;
        let inner = Arc::clone(&self.inner);
        Ok(
            tokio::task::spawn_blocking(move || inner.hover_of(&uri, position))
                .await
                .unwrap_or(None),
        )
    }

    async fn completion(&self, params: CompletionParams) -> RpcResult<Option<CompletionResponse>> {
        let position = params.text_document_position.position;
        let uri = params.text_document_position.text_document.uri;
        let inner = Arc::clone(&self.inner);
        Ok(
            tokio::task::spawn_blocking(move || inner.completion_of(&uri, position))
                .await
                .unwrap_or(None),
        )
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> RpcResult<Option<GotoDefinitionResponse>> {
        let position = params.text_document_position_params.position;
        let uri = params.text_document_position_params.text_document.uri;
        let inner = Arc::clone(&self.inner);
        Ok(
            tokio::task::spawn_blocking(move || inner.definition_of(&uri, position))
                .await
                .unwrap_or(None),
        )
    }

    async fn references(&self, params: ReferenceParams) -> RpcResult<Option<Vec<Location>>> {
        let position = params.text_document_position.position;
        let uri = params.text_document_position.text_document.uri;
        let include_declaration = params.context.include_declaration;
        let inner = Arc::clone(&self.inner);
        Ok(tokio::task::spawn_blocking(move || {
            inner.references_of(&uri, position, include_declaration)
        })
        .await
        .unwrap_or(None))
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> RpcResult<Option<PrepareRenameResponse>> {
        let position = params.position;
        let uri = params.text_document.uri;
        let inner = Arc::clone(&self.inner);
        Ok(
            tokio::task::spawn_blocking(move || inner.prepare_rename_of(&uri, position))
                .await
                .unwrap_or(None),
        )
    }

    async fn rename(&self, params: RenameParams) -> RpcResult<Option<WorkspaceEdit>> {
        let position = params.text_document_position.position;
        let uri = params.text_document_position.text_document.uri;
        let new_name = params.new_name;
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || inner.rename_of(&uri, position, &new_name))
            .await
            .unwrap_or_else(|_| Err("rename task failed".to_string()))
            .map_err(RpcError::invalid_params)
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> RpcResult<Option<DocumentSymbolResponse>> {
        let uri = params.text_document.uri;
        let inner = Arc::clone(&self.inner);
        Ok(tokio::task::spawn_blocking(move || inner.symbols_of(&uri))
            .await
            .unwrap_or(None))
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> RpcResult<Option<WorkspaceSymbolResponse>> {
        let query = params.query;
        let inner = Arc::clone(&self.inner);
        Ok(
            tokio::task::spawn_blocking(move || inner.workspace_symbols_of(&query))
                .await
                .unwrap_or(None),
        )
    }

    async fn folding_range(
        &self,
        params: FoldingRangeParams,
    ) -> RpcResult<Option<Vec<tower_lsp_server::ls_types::FoldingRange>>> {
        let uri = params.text_document.uri;
        let inner = Arc::clone(&self.inner);
        Ok(tokio::task::spawn_blocking(move || inner.folding_of(&uri))
            .await
            .unwrap_or(None))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> RpcResult<Option<SemanticTokensResult>> {
        let uri = params.text_document.uri;
        let inner = Arc::clone(&self.inner);
        Ok(
            tokio::task::spawn_blocking(move || inner.semantic_tokens_full_of(&uri))
                .await
                .unwrap_or(None),
        )
    }

    async fn semantic_tokens_range(
        &self,
        params: SemanticTokensRangeParams,
    ) -> RpcResult<Option<SemanticTokensRangeResult>> {
        let uri = params.text_document.uri;
        let range = params.range;
        let inner = Arc::clone(&self.inner);
        Ok(
            tokio::task::spawn_blocking(move || inner.semantic_tokens_range_of(&uri, range))
                .await
                .unwrap_or(None),
        )
    }

    async fn signature_help(
        &self,
        params: SignatureHelpParams,
    ) -> RpcResult<Option<SignatureHelp>> {
        let position = params.text_document_position_params.position;
        let uri = params.text_document_position_params.text_document.uri;
        let inner = Arc::clone(&self.inner);
        Ok(
            tokio::task::spawn_blocking(move || inner.signature_help_of(&uri, position))
                .await
                .unwrap_or(None),
        )
    }

    async fn code_action(&self, params: CodeActionParams) -> RpcResult<Option<CodeActionResponse>> {
        let uri = params.text_document.uri;
        let range = params.range;
        let inner = Arc::clone(&self.inner);
        Ok(
            tokio::task::spawn_blocking(move || inner.code_actions_of(&uri, range))
                .await
                .unwrap_or(None),
        )
    }

    async fn formatting(
        &self,
        params: DocumentFormattingParams,
    ) -> RpcResult<Option<Vec<TextEdit>>> {
        let uri = params.text_document.uri;
        let inner = Arc::clone(&self.inner);
        Ok(
            tokio::task::spawn_blocking(move || inner.formatting_of(&uri))
                .await
                .unwrap_or(None),
        )
    }

    async fn range_formatting(
        &self,
        params: DocumentRangeFormattingParams,
    ) -> RpcResult<Option<Vec<TextEdit>>> {
        let uri = params.text_document.uri;
        let range = params.range;
        let inner = Arc::clone(&self.inner);
        Ok(
            tokio::task::spawn_blocking(move || inner.range_formatting_of(&uri, range))
                .await
                .unwrap_or(None),
        )
    }

    async fn on_type_formatting(
        &self,
        params: DocumentOnTypeFormattingParams,
    ) -> RpcResult<Option<Vec<TextEdit>>> {
        let position = params.text_document_position.position;
        let uri = params.text_document_position.text_document.uri;
        let ch = params.ch;
        let inner = Arc::clone(&self.inner);
        Ok(
            tokio::task::spawn_blocking(move || inner.on_type_formatting_of(&uri, position, &ch))
                .await
                .unwrap_or(None),
        )
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> RpcResult<Option<Vec<InlayHint>>> {
        let uri = params.text_document.uri;
        let range = params.range;
        let inner = Arc::clone(&self.inner);
        Ok(
            tokio::task::spawn_blocking(move || inner.inlay_hints_of(&uri, range))
                .await
                .unwrap_or(None),
        )
    }

    async fn code_lens(
        &self,
        params: CodeLensParams,
    ) -> RpcResult<Option<Vec<tower_lsp_server::ls_types::CodeLens>>> {
        let uri = params.text_document.uri;
        let inner = Arc::clone(&self.inner);
        Ok(
            tokio::task::spawn_blocking(move || inner.code_lenses_of(&uri))
                .await
                .unwrap_or(None),
        )
    }
}
