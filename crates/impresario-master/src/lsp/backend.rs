//! The language-server backend: state, the debounced analysis worker and the
//! `LanguageServer` implementation (spec section 13).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard, Weak};
use std::time::Duration;

use impresario_core::platforms::Platform;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tower_lsp_server::jsonrpc::Result as RpcResult;
use tower_lsp_server::ls_types::{
    CompletionOptions, CompletionParams, CompletionResponse, DidChangeConfigurationParams,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverParams, HoverProviderCapability, InitializeParams,
    InitializeResult, InitializedParams, OneOf, Position, ServerCapabilities, ServerInfo,
    TextDocumentSyncCapability, TextDocumentSyncKind, Uri,
};
use tower_lsp_server::{Client, LanguageServer};

use super::analyze::{self, Analysis, Overlay};
use super::complete::{self, CompleteCtx};
use super::index::Index;
use super::is_scene_file;
use super::nav;
use super::position::{self, Doc, Encoding};
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
    /// The text of the last analysis, used for range conversion when a file
    /// is not open.
    sources: Mutex<BTreeMap<PathBuf, String>>,
    index: Mutex<Index>,
    /// Distinct platforms of the configured devices (empty = unknown).
    devices: Mutex<Vec<Platform>>,
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
                index: Mutex::new(Index::default()),
                devices: Mutex::new(Vec::new()),
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
        *lock(&self.index) = analysis.index.clone();
        *lock(&self.sources) = analysis.sources.clone();
        let mut platforms = Vec::new();
        for device in &analysis.devices {
            if !platforms.contains(&device.platform) {
                platforms.push(device.platform);
            }
        }
        *lock(&self.devices) = platforms;

        let desired = specs_for(
            &analysis.scenes_dir,
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

    fn hover_of(&self, uri: &Uri, position: Position) -> Option<Hover> {
        let path = path_of(uri)?;
        let text = self.document_text(&path)?;
        let index = lock(&self.index).clone();
        let encoding = *read(&self.encoding);
        let is_scene = is_scene_file(&path);
        nav::hover(&index, &text, is_scene, position, encoding)
    }

    fn completion_of(&self, uri: &Uri, position: Position) -> Option<CompletionResponse> {
        let path = path_of(uri)?;
        let text = self.document_text(&path)?;
        let index = lock(&self.index).clone();
        let platforms = lock(&self.devices).clone();
        let encoding = *read(&self.encoding);
        let is_scene = is_scene_file(&path);
        let doc = Doc::new(&text);
        let (line1, col) = doc.scalar_index(encoding, position)?;
        let ctx = CompleteCtx {
            text: &text,
            line: line1.saturating_sub(1),
            col,
            is_scene,
            index: &index,
            platforms: &platforms,
        };
        complete::complete(&ctx).map(CompletionResponse::Array)
    }

    fn definition_of(
        self: &Arc<Self>,
        uri: &Uri,
        position: Position,
    ) -> Option<GotoDefinitionResponse> {
        let path = path_of(uri)?;
        let text = self.document_text(&path)?;
        let index = lock(&self.index).clone();
        let encoding = *read(&self.encoding);
        let is_scene = is_scene_file(&path);
        let reader = Arc::clone(self);
        let read = move |target: &Path| reader.document_text(target);
        nav::definition(&index, &text, is_scene, position, encoding, &read)
    }

    fn symbols_of(&self, uri: &Uri) -> Option<DocumentSymbolResponse> {
        let path = path_of(uri)?;
        let text = self.document_text(&path)?;
        let encoding = *read(&self.encoding);
        nav::document_symbols(&text, &path, encoding)
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
                document_symbol_provider: Some(OneOf::Left(true)),
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
}
