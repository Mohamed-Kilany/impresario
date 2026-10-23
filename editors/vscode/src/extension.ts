// Impresario editor support.
//
// Two interchangeable front ends share the same editor-neutral language
// service (`crates/impresario-language`):
//
//   * `lsp`        — spawn `impresario lsp` and use vscode-languageclient.
//   * `standalone` — load `media/impresario_language.wasm` (raw ABI) and call
//                    the same service in-process, no child process.
//
// `auto` picks the LSP when the binary is available and falls back to WASM.

import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";
import {
  LanguageClient,
  TransportKind,
  type LanguageClientOptions,
  type ServerOptions,
} from "vscode-languageclient/node";

import {
  LanguageService,
  type CodeAction,
  type Command,
  type DiagnosticInfo,
  type DocumentSymbol,
  type Span,
  type WorkspaceEdit,
  spanToRange,
  servicePositionToVscode,
  vscodePositionToService,
  vscodeRangeToService,
} from "./language-service";

const SELECTOR: vscode.DocumentFilter[] = [
  { language: "impresario-play", scheme: "file" },
  { language: "impresario-scene", scheme: "file" },
  { language: "impresario-theatre", scheme: "file" },
];
const LANGUAGES = new Set([
  "impresario-play",
  "impresario-scene",
  "impresario-theatre",
]);

const TOKEN_TYPES = [
  "namespace",
  "function",
  "property",
  "variable",
  "class",
  "keyword",
];
const TOKEN_MODIFIERS = ["declaration"];
const LEGEND = new vscode.SemanticTokensLegend(TOKEN_TYPES, TOKEN_MODIFIERS);

let client: LanguageClient | undefined;
let standalone: Standalone | undefined;
let status: vscode.StatusBarItem | undefined;
let log: vscode.OutputChannel | undefined;
let starting = false;
const disposables: vscode.Disposable[] = [];

function configuration(): vscode.WorkspaceConfiguration {
  return vscode.workspace.getConfiguration("impresario");
}

function workspaceRoot(): string | undefined {
  const configured = configuration().get<string>("workspace");
  if (configured) {
    return configured;
  }
  const folders = vscode.workspace.workspaceFolders;
  return folders && folders.length > 0 ? folders[0].uri.fsPath : undefined;
}

function resolveAgainstRoot(
  root: string | undefined,
  value: string | undefined
): string | undefined {
  if (!value) {
    return undefined;
  }
  return path.isAbsolute(value) ? value : path.resolve(root ?? "", value);
}

function readText(file: string): string | undefined {
  try {
    return fs.readFileSync(file, "utf8");
  } catch {
    return undefined;
  }
}

function homeFile(name: string): string {
  const home =
    process.env.IMPRESARIO_HOME ?? path.join(os.homedir(), ".impresario");
  return path.join(home, name);
}

/** Resolve an executable name on PATH without spawning anything. */
function onPath(command: string): boolean {
  if (path.isAbsolute(command)) {
    return fs.existsSync(command);
  }
  const extensions =
    process.platform === "win32"
      ? (process.env.PATHEXT ?? ".EXE;.CMD;.BAT;.COM").split(";")
      : [""];
  for (const directory of (process.env.PATH ?? "").split(path.delimiter)) {
    if (!directory) {
      continue;
    }
    for (const extension of extensions) {
      const candidate = path.join(
        directory,
        command + extension.toLowerCase()
      );
      if (fs.existsSync(candidate)) {
        return true;
      }
    }
  }
  return false;
}

function serverAvailable(): boolean {
  return onPath(configuration().get<string>("serverPath") ?? "impresario");
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function show(message: string, kind: "info" | "warning" | "error" = "info"): void {
  const reporter =
    kind === "error"
      ? vscode.window.showErrorMessage
      : kind === "warning"
        ? vscode.window.showWarningMessage
        : vscode.window.showInformationMessage;
  void reporter(`Impresario: ${message}`);
}

async function startLsp(root: string | undefined): Promise<void> {
  const config = configuration();
  const args = ["lsp"];
  if (root) {
    args.push("--workspace", root);
  }
  const configPath = config.get<string>("config");
  const devicesPath = config.get<string>("devices");
  if (configPath) {
    args.push("--config", configPath);
  }
  if (devicesPath) {
    args.push("--devices-config", devicesPath);
  }
  const serverOptions: ServerOptions = {
    command: config.get<string>("serverPath") ?? "impresario",
    args,
    transport: TransportKind.stdio,
  };
  const clientOptions: LanguageClientOptions = {
    documentSelector:
      SELECTOR as unknown as NonNullable<
        LanguageClientOptions["documentSelector"]
      >,
    initializationOptions: {
      impresario: { config: configPath, devices: devicesPath },
    },
  };
  client = new LanguageClient(
    "impresario",
    "Impresario Language Server",
    serverOptions,
    clientOptions
  );
  await client.start();
}

async function startStandalone(root: string | undefined): Promise<void> {
  const configured = configuration().get<string>("standalone.wasmPath");
  const wasmPath = configured
    ? resolveAgainstRoot(root, configured)
    : path.join(__dirname, "..", "media", "impresario_language.wasm");
  if (!wasmPath || !fs.existsSync(wasmPath)) {
    throw new Error(
      `language service not found at ${wasmPath ?? "media/impresario_language.wasm"}; build it with \`npm run build:wasm\``
    );
  }
  const service = await LanguageService.load(wasmPath);
  standalone = new Standalone(service, root);
  await standalone.start();
}

async function start(): Promise<void> {
  if (starting) {
    return;
  }
  starting = true;
  try {
    const root = workspaceRoot();
    const configured = configuration().get<string>("mode") ?? "auto";
    let mode = configured;
    if (mode === "auto") {
      mode = serverAvailable() ? "lsp" : "standalone";
    }
    try {
      if (mode === "lsp") {
        await startLsp(root);
      } else {
        await startStandalone(root);
      }
    } catch (error) {
      if (client) {
        try {
          await client.stop();
        } catch {
          // The failed client may never have reached the running state.
        }
        client = undefined;
      }
      if (configured !== "auto" || mode === "standalone") {
        throw error;
      }
      log?.appendLine(
        `LSP failed (${errorMessage(error)}); using standalone mode`
      );
      await startStandalone(root);
      mode = "standalone";
    }
    if (status) {
      status.text = `$(beaker) Impresario: ${mode === "lsp" ? "LSP" : "standalone"}`;
      status.tooltip = "Click to restart Impresario editor support";
      status.command = "impresario.restart";
      status.show();
    }
  } catch (error) {
    show(errorMessage(error), "error");
    if (status) {
      status.text = "$(error) Impresario: unavailable";
      status.tooltip = errorMessage(error);
      status.show();
    }
  } finally {
    starting = false;
  }
}

async function stop(): Promise<void> {
  if (client) {
    const running = client;
    client = undefined;
    try {
      await running.stop();
    } catch {
      // A server that already exited needs no further stop.
    }
  }
  if (standalone) {
    standalone.dispose();
    standalone = undefined;
  }
}

async function restart(): Promise<void> {
  await stop();
  await start();
}

/** The in-process VS Code adapter around the shared language service. */
class Standalone {
  readonly service: LanguageService;
  private readonly root: string | undefined;
  private files = new Map<string, string>();
  private readonly overlays = new Map<string, string>();
  private readonly diagnostics: vscode.DiagnosticCollection;
  private readonly tokens = new vscode.EventEmitter<void>();
  private readonly lenses = new vscode.EventEmitter<void>();
  private readonly hints = new vscode.EventEmitter<void>();
  private readonly ownDisposables: vscode.Disposable[] = [];
  private timer: ReturnType<typeof setTimeout> | undefined;
  private warned = false;

  constructor(service: LanguageService, root: string | undefined) {
    this.service = service;
    this.root = root;
    this.diagnostics = vscode.languages.createDiagnosticCollection("impresario");
  }

  async start(): Promise<void> {
    await this.refreshFiles();
    this.register();
    this.watch();
    await this.analyze();
  }

  dispose(): void {
    if (this.timer) {
      clearTimeout(this.timer);
      this.timer = undefined;
    }
    for (const disposable of this.ownDisposables.splice(0)) {
      disposable.dispose();
    }
    this.tokens.dispose();
    this.lenses.dispose();
    this.hints.dispose();
    this.diagnostics.dispose();
  }

  textOf(file: string): string | undefined {
    if (this.overlays.has(file)) {
      return this.overlays.get(file);
    }
    if (this.files.has(file)) {
      return this.files.get(file);
    }
    return readText(file);
  }

  async refreshFiles(): Promise<void> {
    if (!this.root) {
      for (const document of vscode.workspace.textDocuments) {
        if (LANGUAGES.has(document.languageId)) {
          this.files.set(document.uri.fsPath, document.getText());
        }
      }
      return;
    }
    const pattern = new vscode.RelativePattern(
      this.root,
      "**/*.{scene,theatre,play}"
    );
    const uris = await vscode.workspace.findFiles(
      pattern,
      "**/{node_modules,.git,out,target,.impresario}/**"
    );
    const next = new Map<string, string>();
    for (const uri of uris) {
      const file = uri.fsPath;
      const cached = this.files.get(file);
      if (cached !== undefined) {
        next.set(file, cached);
      } else {
        try {
          const bytes = await vscode.workspace.fs.readFile(uri);
          next.set(
            file,
            new TextDecoder("utf-8", { fatal: true }).decode(bytes)
          );
        } catch {
          // A file removed between findFiles and read just disappears.
        }
      }
    }
    this.files = next;
  }

  private configTexts(): {
    devices: string;
    devicesText: string | undefined;
    apps: string;
    appsText: string | undefined;
  } {
    const config = configuration();
    const devices =
      resolveAgainstRoot(this.root, config.get<string>("devices")) ??
      path.join(this.root ?? "", "impresario.devices.config");
    const apps =
      resolveAgainstRoot(this.root, config.get<string>("apps")) ??
      path.join(this.root ?? "", "impresario.apps.config");
    const devicesText =
      readText(devices) ?? readText(homeFile("devices.yaml"));
    const appsText = readText(apps) ?? readText(homeFile("apps.yaml"));
    return { devices, devicesText, apps, appsText };
  }

  async analyze(): Promise<void> {
    this.timer = undefined;
    try {
      await this.refreshFiles();
      const config = this.configTexts();
      const sources = new Map(this.files);
      for (const [file, text] of this.overlays) {
        sources.set(file, text);
      }
      const result = this.service.analyze({
        sources: [...sources].map(([file, text]) => ({ path: file, text })),
        root: this.root,
        devicesYaml: config.devicesText,
        devicesPath: config.devices,
        appsYaml: config.appsText,
        appsPath: config.apps,
      });
      this.publish(result.diagnostics ?? {});
      this.tokens.fire();
      this.lenses.fire();
      this.hints.fire();
      this.warned = false;
    } catch (error) {
      if (!this.warned) {
        this.warned = true;
        log?.appendLine(
          `standalone analysis failed: ${
            error instanceof Error ? error.stack ?? error.message : String(error)
          }`
        );
        show(`standalone analysis failed: ${errorMessage(error)}`, "error");
      }
    }
  }

  schedule(): void {
    if (this.timer) {
      clearTimeout(this.timer);
    }
    this.timer = setTimeout(() => void this.analyze(), 150);
  }

  private publish(byFile: Record<string, DiagnosticInfo[]>): void {
    this.diagnostics.clear();
    for (const [file, infos] of Object.entries(byFile)) {
      const text = this.textOf(file);
      if (!text) {
        continue;
      }
      const uri = vscode.Uri.file(file);
      const list = infos
        .map((info) => {
          const range = spanToRange(text, info.span);
          let message = info.message;
          if (info.help) {
            message += `\nhelp: ${info.help}`;
          }
          const severity =
            info.severity === "error"
              ? vscode.DiagnosticSeverity.Error
              : info.severity === "warning"
                ? vscode.DiagnosticSeverity.Warning
                : vscode.DiagnosticSeverity.Information;
          const diagnostic = new vscode.Diagnostic(
            new vscode.Range(
              range.start.line,
              range.start.character,
              range.end.line,
              range.end.character
            ),
            message,
            severity
          );
          diagnostic.code = info.code;
          diagnostic.source = "impresario";
          const related: vscode.DiagnosticRelatedInformation[] = [];
          for (const item of info.related) {
            const relatedText = this.textOf(item.span.file);
            if (!relatedText) {
              continue;
            }
            const relatedRange = spanToRange(relatedText, item.span);
            related.push(
              new vscode.DiagnosticRelatedInformation(
                new vscode.Location(
                  vscode.Uri.file(item.span.file),
                  new vscode.Range(
                    relatedRange.start.line,
                    relatedRange.start.character,
                    relatedRange.end.line,
                    relatedRange.end.character
                  )
                ),
                item.message
              )
            );
          }
          if (related.length > 0) {
            diagnostic.relatedInformation = related;
          }
          return diagnostic;
        })
        .filter((diagnostic) => diagnostic !== undefined);
      if (list.length > 0) {
        this.diagnostics.set(uri, list);
      }
    }
  }

  private watch(): void {
    const watcher = vscode.workspace.createFileSystemWatcher(
      "**/*.{scene,theatre,play}",
      false,
      false,
      false
    );
    this.ownDisposables.push(
      watcher,
      watcher.onDidCreate((uri) => {
        this.files.delete(uri.fsPath);
        this.schedule();
      }),
      watcher.onDidChange(async (uri) => {
        if (!this.overlays.has(uri.fsPath)) {
          try {
            const bytes = await vscode.workspace.fs.readFile(uri);
            this.files.set(
              uri.fsPath,
              new TextDecoder("utf-8", { fatal: true }).decode(bytes)
            );
          } catch {
            // The next refresh reconciles the map.
          }
        }
        this.schedule();
      }),
      watcher.onDidDelete((uri) => {
        this.files.delete(uri.fsPath);
        this.overlays.delete(uri.fsPath);
        this.schedule();
      })
    );
    const config = vscode.workspace.createFileSystemWatcher(
      "**/{impresario.config,impresario.devices.config,impresario.apps.config}"
    );
    this.ownDisposables.push(
      config,
      config.onDidCreate(() => this.schedule()),
      config.onDidChange(() => this.schedule()),
      config.onDidDelete(() => this.schedule())
    );
    this.ownDisposables.push(
      vscode.workspace.onDidOpenTextDocument((document) => {
        if (LANGUAGES.has(document.languageId)) {
          this.overlays.set(document.uri.fsPath, document.getText());
          this.schedule();
        }
      }),
      vscode.workspace.onDidChangeTextDocument((event) => {
        if (!LANGUAGES.has(event.document.languageId)) {
          return;
        }
        this.overlays.set(event.document.uri.fsPath, event.document.getText());
        this.schedule();
      }),
      vscode.workspace.onDidCloseTextDocument((document) => {
        if (LANGUAGES.has(document.languageId)) {
          this.overlays.delete(document.uri.fsPath);
          this.files.delete(document.uri.fsPath);
          this.schedule();
        }
      }),
      vscode.workspace.onDidSaveTextDocument((document) => {
        if (LANGUAGES.has(document.languageId)) {
          this.overlays.delete(document.uri.fsPath);
          this.files.set(document.uri.fsPath, document.getText());
          this.schedule();
        }
      })
    );
  }

  private position(
    document: vscode.TextDocument,
    position: vscode.Position
  ): { line: number; col: number } {
    return vscodePositionToService(document.getText(), position);
  }

  private range(text: string, span: Span): vscode.Range {
    const range = spanToRange(text, span);
    return new vscode.Range(
      range.start.line,
      range.start.character,
      range.end.line,
      range.end.character
    );
  }

  locations(spans: Span[] | null | undefined): vscode.Location[] {
    const locations: vscode.Location[] = [];
    for (const span of spans ?? []) {
      const text = this.textOf(span.file);
      if (!text) {
        continue;
      }
      locations.push(
        new vscode.Location(vscode.Uri.file(span.file), this.range(text, span))
      );
    }
    return locations;
  }

  private workspaceEdit(edit: WorkspaceEdit | null | undefined): vscode.WorkspaceEdit {
    const workspace = new vscode.WorkspaceEdit();
    if (!edit) {
      return workspace;
    }
    const created = new Map<string, string>();
    for (const file of edit.create_files) {
      const uri = vscode.Uri.file(file.path);
      workspace.createFile(uri, {
        overwrite: Boolean(file.overwrite),
        ignoreIfExists: !file.overwrite,
      });
      created.set(file.path, file.text);
      if (file.text) {
        workspace.insert(uri, new vscode.Position(0, 0), file.text);
      }
    }
    for (const file of edit.rename_files) {
      workspace.renameFile(
        vscode.Uri.file(file.old_path),
        vscode.Uri.file(file.new_path)
      );
    }
    for (const file of edit.delete_files) {
      workspace.deleteFile(vscode.Uri.file(file));
    }
    for (const [file, edits] of Object.entries(edit.changes)) {
      const text = created.get(file) ?? this.textOf(file);
      if (!text) {
        continue;
      }
      const uri = vscode.Uri.file(file);
      for (const change of edits) {
        workspace.replace(uri, this.range(text, change.span), change.new_text);
      }
    }
    return workspace;
  }

  private commands(commands: Command[]): vscode.Command[] {
    return commands.map((command) => ({
      title: command.title,
      command: command.command,
      arguments: command.arguments,
    }));
  }

  private register(): void {
    const self = this;
    this.ownDisposables.push(
      vscode.languages.registerCompletionItemProvider(
        SELECTOR,
        {
          provideCompletionItems(document, position) {
            const text = document.getText();
            const items = self.service.completion(
              document.uri.fsPath,
              self.position(document, position)
            );
            return new vscode.CompletionList(
              items.map((item) => {
                const completion = new vscode.CompletionItem(
                  item.label,
                  completionKind(item.kind)
                );
                completion.detail = item.detail ?? undefined;
                if (item.documentation) {
                  completion.documentation = new vscode.MarkdownString(
                    item.documentation
                  );
                }
                completion.sortText = item.sort_text ?? undefined;
                completion.filterText = item.filter_text ?? undefined;
                if (item.replace) {
                  completion.range = self.range(text, item.replace.span);
                  completion.insertText = item.replace.new_text;
                } else if (item.insert_text) {
                  completion.insertText = item.insert_text;
                }
                return completion;
              }),
              false
            );
          },
        },
        ".",
        ":",
        "(",
        "$",
        "{",
        "|"
      ),
      vscode.languages.registerHoverProvider(SELECTOR, {
        provideHover(document, position) {
          const hover = self.service.hover(
            document.uri.fsPath,
            self.position(document, position)
          );
          if (!hover) {
            return undefined;
          }
          const contents = new vscode.MarkdownString(hover.markdown);
          contents.supportHtml = false;
          if (hover.span) {
            return new vscode.Hover(
              contents,
              self.range(document.getText(), hover.span)
            );
          }
          return new vscode.Hover(contents);
        },
      }),
      vscode.languages.registerDefinitionProvider(SELECTOR, {
        provideDefinition(document, position) {
          const spans = self.service.definition(
            document.uri.fsPath,
            self.position(document, position)
          );
          const locations = self.locations(spans);
          return locations.length === 1 ? locations[0] : locations;
        },
      }),
      vscode.languages.registerReferenceProvider(SELECTOR, {
        provideReferences(document, position, context) {
          const spans = self.service.references(
            document.uri.fsPath,
            self.position(document, position),
            context.includeDeclaration
          );
          return self.locations(spans);
        },
      }),
      vscode.languages.registerRenameProvider(SELECTOR, {
        prepareRename(document, position) {
          const prepared = self.service.prepareRename(
            document.uri.fsPath,
            self.position(document, position)
          );
          if (!prepared) {
            return undefined;
          }
          return {
            range: self.range(document.getText(), prepared.span),
            placeholder: prepared.placeholder,
          };
        },
        provideRenameEdits(document, position, newName) {
          const edit = self.service.rename(
            document.uri.fsPath,
            self.position(document, position),
            newName
          );
          return self.workspaceEdit(edit);
        },
      }),
      vscode.languages.registerDocumentSymbolProvider(SELECTOR, {
        provideDocumentSymbols(document) {
          const text = document.getText();
          const convert = (symbol: DocumentSymbol): vscode.DocumentSymbol => {
            const symbolInfo = new vscode.DocumentSymbol(
              symbol.name,
              symbol.detail ?? "",
              symbolKind(symbol.kind),
              self.range(text, symbol.span),
              self.range(text, symbol.selection_span)
            );
            symbolInfo.children = symbol.children.map(convert);
            return symbolInfo;
          };
          return self.service
            .documentSymbols(document.uri.fsPath)
            .map(convert);
        },
      }),
      vscode.languages.registerWorkspaceSymbolProvider({
        provideWorkspaceSymbols(query) {
          return self.service
            .workspaceSymbols(query)
            .flatMap((symbol): vscode.SymbolInformation[] => {
              const text = self.textOf(symbol.span.file);
              if (!text) {
                return [];
              }
              return [
                new vscode.SymbolInformation(
                  symbol.name,
                  symbolKind(symbol.kind),
                  symbol.detail ?? "",
                  new vscode.Location(
                    vscode.Uri.file(symbol.span.file),
                    self.range(text, symbol.span)
                  )
                ),
              ];
            });
        },
      }),
      vscode.languages.registerFoldingRangeProvider(SELECTOR, {
        provideFoldingRanges(document) {
          return self.service.folding(document.uri.fsPath).map((range) => {
            const folding = new vscode.FoldingRange(
              Math.max(0, range.start_line - 1),
              Math.max(0, range.end_line - 1)
            );
            if (range.kind === "comment") {
              folding.kind = vscode.FoldingRangeKind.Comment;
            } else if (range.kind === "imports") {
              folding.kind = vscode.FoldingRangeKind.Imports;
            } else if (range.kind === "region") {
              folding.kind = vscode.FoldingRangeKind.Region;
            }
            return folding;
          });
        },
      }),
      vscode.languages.registerSignatureHelpProvider(
        SELECTOR,
        {
          provideSignatureHelp(document, position) {
            const help = self.service.signatureHelp(
              document.uri.fsPath,
              self.position(document, position)
            );
            if (!help) {
              return undefined;
            }
            const result = new vscode.SignatureHelp();
            result.signatures = help.signatures.map((signature) => {
              const information = new vscode.SignatureInformation(
                signature.label,
                signature.documentation
                  ? new vscode.MarkdownString(signature.documentation)
                  : undefined
              );
              information.parameters = signature.parameters.map(
                (parameter) =>
                  new vscode.ParameterInformation(
                    parameter.label,
                    parameter.documentation ?? undefined
                  )
              );
              return information;
            });
            result.activeSignature = help.active_signature;
            result.activeParameter = help.active_parameter;
            return result;
          },
        },
        "(",
        ","
      ),
      vscode.languages.registerCodeActionsProvider(
        SELECTOR,
        {
          provideCodeActions(document, range) {
            const text = document.getText();
            const selection = vscodeRangeToService(text, range);
            const actions = self.service.codeActions(
              document.uri.fsPath,
              selection.start,
              selection.end
            );
            const diagnostics = vscode.languages
              .getDiagnostics(document.uri)
              .filter(
                (diagnostic) => typeof diagnostic.code === "string"
              );
            return actions.map((action: CodeAction) => {
              const codeAction = new vscode.CodeAction(
                action.title,
                codeActionKind(action.kind)
              );
              codeAction.diagnostics = diagnostics.filter((diagnostic) =>
                action.diagnostics.includes(diagnostic.code as string)
              );
              if (action.edit) {
                codeAction.edit = self.workspaceEdit(action.edit);
              }
              if (action.command) {
                codeAction.command = self.commands([action.command])[0];
              }
              return codeAction;
            });
          },
        },
        {
          providedCodeActionKinds: [
            vscode.CodeActionKind.QuickFix,
            vscode.CodeActionKind.RefactorExtract,
            vscode.CodeActionKind.RefactorRewrite,
          ],
        }
      ),
      vscode.languages.registerDocumentFormattingEditProvider(SELECTOR, {
        provideDocumentFormattingEdits(document) {
          try {
            const edit = self.service.format(document.uri.fsPath);
            return edit
              ? [
                  new vscode.TextEdit(
                    self.range(document.getText(), edit.span),
                    edit.new_text
                  ),
                ]
              : [];
          } catch (error) {
            log?.appendLine(`format failed: ${errorMessage(error)}`);
            return [];
          }
        },
      }),
      vscode.languages.registerDocumentRangeFormattingEditProvider(SELECTOR, {
        provideDocumentRangeFormattingEdits(document, range) {
          try {
            const selection = vscodeRangeToService(
              document.getText(),
              range
            );
            return self.service
              .rangeFormatting(
                document.uri.fsPath,
                selection.start,
                selection.end
              )
              .map(
                (edit) =>
                  new vscode.TextEdit(
                    self.range(document.getText(), edit.span),
                    edit.new_text
                  )
              );
          } catch (error) {
            log?.appendLine(`range format failed: ${errorMessage(error)}`);
            return [];
          }
        },
      }),
      vscode.languages.registerOnTypeFormattingEditProvider(
        SELECTOR,
        {
          provideOnTypeFormattingEdits(document, position, ch) {
            try {
              const edits = self.service.onTypeFormatting(
                document.uri.fsPath,
                self.position(document, position),
                ch
              );
              return edits.map(
                (edit) =>
                  new vscode.TextEdit(
                    self.range(document.getText(), edit.span),
                    edit.new_text
                  )
              );
            } catch (error) {
              log?.appendLine(`on-type format failed: ${errorMessage(error)}`);
              return [];
            }
          },
        },
        "|",
        "\n"
      ),
      vscode.languages.registerInlayHintsProvider(SELECTOR, {
        onDidChangeInlayHints: self.hints.event,
        provideInlayHints(document, range) {
          return self.service
            .inlayHints(
              document.uri.fsPath,
              range.start.line + 1,
              range.end.line + 1
            )
            .map((hint) => {
              const position = servicePositionToVscode(
                document.getText(),
                hint.span
              );
              const inlay = new vscode.InlayHint(
                new vscode.Position(position.line, position.character),
                hint.label,
                hint.kind === "parameter"
                  ? vscode.InlayHintKind.Parameter
                  : vscode.InlayHintKind.Type
              );
              inlay.paddingLeft = false;
              return inlay;
            });
        },
      }),
      vscode.languages.registerCodeLensProvider(SELECTOR, {
        onDidChangeCodeLenses: self.lenses.event,
        provideCodeLenses(document) {
          return self.service
            .codeLenses(document.uri.fsPath)
            .map(
              (lens) =>
                new vscode.CodeLens(
                  self.range(document.getText(), lens.span),
                  self.commands([lens.command])[0]
                )
            );
        },
      }),
      vscode.languages.registerDocumentSemanticTokensProvider(
        SELECTOR,
        {
          onDidChangeSemanticTokens: self.tokens.event,
          provideDocumentSemanticTokens(document) {
            const text = document.getText();
            const builder = new vscode.SemanticTokensBuilder(LEGEND);
            for (const token of self.service.semanticTokens(
              document.uri.fsPath
            )) {
              const range = spanToRange(text, token.span);
              const type = TOKEN_TYPES.indexOf(token.token_type);
              if (type < 0 || range.start.line !== range.end.line) {
                continue;
              }
              let modifiers = 0;
              for (const modifier of token.modifiers) {
                const index = TOKEN_MODIFIERS.indexOf(modifier);
                if (index >= 0) {
                  modifiers |= 1 << index;
                }
              }
              builder.push(
                range.start.line,
                range.start.character,
                range.end.character - range.start.character,
                type,
                modifiers
              );
            }
            return builder.build();
          },
        },
        LEGEND
      ),
      vscode.languages.registerDocumentRangeSemanticTokensProvider(
        SELECTOR,
        {
          provideDocumentRangeSemanticTokens(document, range) {
            const text = document.getText();
            const start = range.start.line + 1;
            const end = range.end.line + 1;
            const builder = new vscode.SemanticTokensBuilder(LEGEND);
            for (const token of self.service.semanticTokens(
              document.uri.fsPath
            )) {
              if (token.span.line < start || token.span.line > end) {
                continue;
              }
              const tokenRange = spanToRange(text, token.span);
              const type = TOKEN_TYPES.indexOf(token.token_type);
              if (
                type < 0 ||
                tokenRange.start.line !== tokenRange.end.line
              ) {
                continue;
              }
              let modifiers = 0;
              for (const modifier of token.modifiers) {
                const index = TOKEN_MODIFIERS.indexOf(modifier);
                if (index >= 0) {
                  modifiers |= 1 << index;
                }
              }
              builder.push(
                tokenRange.start.line,
                tokenRange.start.character,
                tokenRange.end.character - tokenRange.start.character,
                type,
                modifiers
              );
            }
            return builder.build();
          },
        },
        LEGEND
      )
    );
  }
}

function completionKind(kind: string): vscode.CompletionItemKind {
  const kinds: Record<string, vscode.CompletionItemKind> = {
    file: vscode.CompletionItemKind.File,
    function: vscode.CompletionItemKind.Function,
    property: vscode.CompletionItemKind.Property,
    reference: vscode.CompletionItemKind.Reference,
    constant: vscode.CompletionItemKind.Constant,
    class: vscode.CompletionItemKind.Class,
    value: vscode.CompletionItemKind.Value,
    variable: vscode.CompletionItemKind.Variable,
    keyword: vscode.CompletionItemKind.Keyword,
    namespace: vscode.CompletionItemKind.Module,
    enumMember: vscode.CompletionItemKind.EnumMember,
    operator: vscode.CompletionItemKind.Operator,
    snippet: vscode.CompletionItemKind.Snippet,
  };
  return kinds[kind] ?? vscode.CompletionItemKind.Text;
}

function symbolKind(kind: string): vscode.SymbolKind {
  const kinds: Record<string, vscode.SymbolKind> = {
    file: vscode.SymbolKind.File,
    module: vscode.SymbolKind.Module,
    namespace: vscode.SymbolKind.Namespace,
    package: vscode.SymbolKind.Package,
    class: vscode.SymbolKind.Class,
    method: vscode.SymbolKind.Method,
    property: vscode.SymbolKind.Property,
    field: vscode.SymbolKind.Field,
    constructor: vscode.SymbolKind.Constructor,
    enum: vscode.SymbolKind.Enum,
    interface: vscode.SymbolKind.Interface,
    function: vscode.SymbolKind.Function,
    variable: vscode.SymbolKind.Variable,
    constant: vscode.SymbolKind.Constant,
    string: vscode.SymbolKind.String,
    number: vscode.SymbolKind.Number,
    boolean: vscode.SymbolKind.Boolean,
    array: vscode.SymbolKind.Array,
    object: vscode.SymbolKind.Object,
    key: vscode.SymbolKind.Key,
    null: vscode.SymbolKind.Null,
    enumMember: vscode.SymbolKind.EnumMember,
    struct: vscode.SymbolKind.Struct,
    event: vscode.SymbolKind.Event,
    operator: vscode.SymbolKind.Operator,
    typeParameter: vscode.SymbolKind.TypeParameter,
  };
  return kinds[kind] ?? vscode.SymbolKind.Object;
}

function codeActionKind(kind: string): vscode.CodeActionKind {
  if (kind === "quickfix") {
    return vscode.CodeActionKind.QuickFix;
  }
  if (kind === "refactor.extract") {
    return vscode.CodeActionKind.RefactorExtract;
  }
  if (kind === "refactor.rewrite") {
    return vscode.CodeActionKind.RefactorRewrite;
  }
  if (kind === "source.fixAll") {
    return vscode.CodeActionKind.SourceFixAll;
  }
  return vscode.CodeActionKind.QuickFix;
}

function registerCommands(): void {
  disposables.push(
    vscode.commands.registerCommand("impresario.restart", () => restart()),
    vscode.commands.registerCommand(
      "impresario.runAct",
      (file?: string, act?: string) => {
        if (!file || !act) {
          return;
        }
        const command = configuration().get<string>("serverPath") ?? "impresario";
        const terminal = vscode.window.createTerminal({
          name: `Impresario: ${act}`,
          cwd: workspaceRoot(),
        });
        terminal.show();
        terminal.sendText(
          `${command} run --play "${file.replace(/"/g, '\\"')}" --act "${act.replace(/"/g, '\\"')}"`
        );
      }
    ),
    vscode.commands.registerCommand(
      "impresario.showReferences",
      async (file?: string, line?: number, character?: number) => {
        if (!standalone || !file || line === undefined || character === undefined) {
          return;
        }
        const spans = standalone.service.references(
          file,
          { line: line + 1, col: character + 1 },
          true
        );
        const locations = standalone.locations(spans);
        await vscode.commands.executeCommand(
          "editor.action.showReferences",
          vscode.Uri.file(file),
          new vscode.Position(line, character),
          locations
        );
      }
    )
  );
}

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  log = vscode.window.createOutputChannel("Impresario");
  status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 0);
  disposables.push(log, status);
  registerCommands();
  context.subscriptions.push(
    ...disposables,
    {
      dispose() {
        void stop();
      },
    },
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (
        event.affectsConfiguration("impresario.mode") ||
        event.affectsConfiguration("impresario.serverPath") ||
        event.affectsConfiguration("impresario.workspace")
      ) {
        show(
          "configuration changed; run Impresario: Restart Language Support"
        );
      }
    })
  );
  await start();
}

export async function deactivate(): Promise<void> {
  await stop();
}
