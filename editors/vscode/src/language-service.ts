// Loader and position helpers for the raw-ABI WASM language service.
//
// The Rust module is `crates/impresario-language-wasm`; it deliberately has
// no wasm-bindgen dependency. `impresario_invoke` returns a packed u64:
// high 32 bits are the response length, low 32 bits the response pointer.

import * as fs from "fs";

export interface Span {
  file: string;
  line: number;
  col: number;
  len: number;
}

export interface ServicePosition {
  line: number;
  col: number;
}

export interface VscodePosition {
  line: number;
  character: number;
}

export interface VscodeRange {
  start: VscodePosition;
  end: VscodePosition;
}

export interface TextEdit {
  span: Span;
  new_text: string;
}

export interface CreateFile {
  path: string;
  text: string;
  overwrite: boolean;
}

export interface RenameFile {
  old_path: string;
  new_path: string;
}

export interface WorkspaceEdit {
  changes: Record<string, TextEdit[]>;
  create_files: CreateFile[];
  delete_files: string[];
  rename_files: RenameFile[];
}

export interface CompletionItem {
  label: string;
  kind: string;
  detail?: string | null;
  insert_text?: string | null;
  sort_text?: string | null;
  filter_text?: string | null;
  documentation?: string | null;
  replace?: TextEdit | null;
}

export interface HoverResult {
  markdown: string;
  span?: Span | null;
}

export interface DocumentSymbol {
  name: string;
  detail?: string | null;
  kind: string;
  span: Span;
  selection_span: Span;
  children: DocumentSymbol[];
}

export interface WorkspaceSymbol {
  name: string;
  detail?: string | null;
  kind: string;
  span: Span;
}

export interface FoldingRange {
  start_line: number;
  end_line: number;
  kind?: string | null;
}

export interface SemanticToken {
  span: Span;
  token_type: string;
  modifiers: string[];
}

export interface SignatureParameter {
  label: string;
  documentation?: string | null;
}

export interface Signature {
  label: string;
  documentation?: string | null;
  parameters: SignatureParameter[];
}

export interface SignatureHelp {
  signatures: Signature[];
  active_signature: number;
  active_parameter: number;
}

export interface Command {
  title: string;
  command: string;
  arguments: unknown[];
}

export interface CodeAction {
  title: string;
  kind: string;
  diagnostics: string[];
  edit?: WorkspaceEdit | null;
  command?: Command | null;
}

export interface InlayHint {
  span: Span;
  label: string;
  kind: string;
}

export interface CodeLens {
  span: Span;
  command: Command;
}

export interface PrepareRename {
  span: Span;
  placeholder: string;
}

export interface RelatedInfo {
  span: Span;
  message: string;
}

export interface DiagnosticInfo {
  code: string;
  severity: string;
  message: string;
  help?: string | null;
  span: Span;
  related: RelatedInfo[];
}

export interface SourceFile {
  path: string;
  text: string;
}

export interface AnalyzeRequest {
  sources: SourceFile[];
  root?: string;
  devicesYaml?: string;
  devicesPath?: string;
  appsYaml?: string;
  appsPath?: string;
  devices?: unknown;
  apps?: unknown;
}

export interface AnalyzeResult {
  diagnostics: Record<string, DiagnosticInfo[]>;
}

interface ImpresarioExports extends WebAssembly.Exports {
  memory: WebAssembly.Memory;
  impresario_alloc(length: number): number;
  impresario_free(pointer: number, length: number): void;
  impresario_invoke(pointer: number, length: number): bigint;
}

interface Response<T> {
  ok: boolean;
  result?: T;
  error?: string;
}

export class LanguageService {
  private readonly exports: ImpresarioExports;
  private readonly memory: WebAssembly.Memory;
  private readonly encoder = new TextEncoder();
  private readonly decoder = new TextDecoder("utf-8", { fatal: true });

  constructor(instance: WebAssembly.Instance) {
    this.exports = instance.exports as unknown as ImpresarioExports;
    this.memory = this.exports.memory;
  }

  static async load(wasmPath: string): Promise<LanguageService> {
    const bytes = fs.readFileSync(wasmPath);
    const { instance } = await WebAssembly.instantiate(bytes, {});
    return new LanguageService(instance);
  }

  /** Send one request and return the `result` payload. */
  call<T>(request: Record<string, unknown>): T {
    const input = this.encoder.encode(JSON.stringify(request));
    const inputPointer = this.exports.impresario_alloc(input.length);
    if (!inputPointer) {
      throw new Error("impresario WASM allocation failed");
    }
    try {
      new Uint8Array(this.memory.buffer, inputPointer, input.length).set(input);
      const packed = this.exports.impresario_invoke(
        inputPointer,
        input.length
      );
      const outputPointer = Number(packed & 0xffffffffn);
      const outputLength = Number(packed >> 32n);
      try {
        const bytes = new Uint8Array(
          this.memory.buffer,
          outputPointer,
          outputLength
        );
        const response = JSON.parse(this.decoder.decode(bytes)) as Response<T>;
        if (!response.ok) {
          throw new Error(response.error ?? "language service error");
        }
        return response.result as T;
      } finally {
        this.exports.impresario_free(outputPointer, outputLength);
      }
    } finally {
      this.exports.impresario_free(inputPointer, input.length);
    }
  }

  analyze(request: AnalyzeRequest): AnalyzeResult {
    return this.call<AnalyzeResult>({ op: "analyze", ...request });
  }

  completion(path: string, position: ServicePosition): CompletionItem[] {
    return (
      this.call<CompletionItem[]>({ op: "completion", path, position }) ?? []
    );
  }

  hover(path: string, position: ServicePosition): HoverResult | null {
    return this.call<HoverResult | null>({ op: "hover", path, position });
  }

  definition(path: string, position: ServicePosition): Span[] {
    return (
      this.call<Span[]>({ op: "definition", path, position }) ?? []
    );
  }

  references(
    path: string,
    position: ServicePosition,
    includeDeclaration = true
  ): Span[] {
    return (
      this.call<Span[]>({
        op: "references",
        path,
        position,
        includeDeclaration,
      }) ?? []
    );
  }

  prepareRename(
    path: string,
    position: ServicePosition
  ): PrepareRename | null {
    return this.call<PrepareRename | null>({
      op: "prepareRename",
      path,
      position,
    });
  }

  rename(
    path: string,
    position: ServicePosition,
    newName: string
  ): WorkspaceEdit {
    return this.call<WorkspaceEdit>({
      op: "rename",
      path,
      position,
      newName,
    });
  }

  documentSymbols(path: string): DocumentSymbol[] {
    return (
      this.call<DocumentSymbol[]>({ op: "documentSymbols", path }) ?? []
    );
  }

  workspaceSymbols(query = ""): WorkspaceSymbol[] {
    return (
      this.call<WorkspaceSymbol[]>({ op: "workspaceSymbols", query }) ?? []
    );
  }

  folding(path: string): FoldingRange[] {
    return this.call<FoldingRange[]>({ op: "folding", path }) ?? [];
  }

  semanticTokens(path: string): SemanticToken[] {
    return (
      this.call<SemanticToken[]>({ op: "semanticTokens", path }) ?? []
    );
  }

  signatureHelp(
    path: string,
    position: ServicePosition
  ): SignatureHelp | null {
    return this.call<SignatureHelp | null>({
      op: "signatureHelp",
      path,
      position,
    });
  }

  codeActions(
    path: string,
    start: ServicePosition,
    end: ServicePosition
  ): CodeAction[] {
    return (
      this.call<CodeAction[]>({ op: "codeActions", path, start, end }) ?? []
    );
  }

  format(path: string): TextEdit | null {
    return this.call<TextEdit | null>({ op: "format", path });
  }

  rangeFormatting(
    path: string,
    start: ServicePosition,
    end: ServicePosition
  ): TextEdit[] {
    return (
      this.call<TextEdit[]>({
        op: "rangeFormatting",
        path,
        start,
        end,
      }) ?? []
    );
  }

  onTypeFormatting(
    path: string,
    position: ServicePosition,
    ch: string
  ): TextEdit[] {
    return (
      this.call<TextEdit[]>({
        op: "onTypeFormatting",
        path,
        position,
        ch,
      }) ?? []
    );
  }

  inlayHints(
    path: string,
    startLine: number,
    endLine: number
  ): InlayHint[] {
    return (
      this.call<InlayHint[]>({
        op: "inlayHints",
        path,
        startLine,
        endLine,
      }) ?? []
    );
  }

  codeLenses(path: string): CodeLens[] {
    return this.call<CodeLens[]>({ op: "codeLenses", path }) ?? [];
  }

  diagnostics(path: string): DiagnosticInfo[] {
    return (
      this.call<DiagnosticInfo[]>({ op: "diagnostics", path }) ?? []
    );
  }
}

/** Split like Rust's `str::lines()`: on `\n`, dropping a trailing `\r`. */
export function lineTexts(text: string): string[] {
  return text
    .split("\n")
    .map((line) => (line.endsWith("\r") ? line.slice(0, -1) : line));
}

/** UTF-16 code units from the start of a line to a 1-based scalar column. */
export function utf16Column(line: string, scalarColumn: number): number {
  const scalars = Array.from(line);
  const end = Math.max(0, Math.min(scalars.length, scalarColumn - 1));
  return scalars.slice(0, end).join("").length;
}

/** Convert a 1-based service position to a 0-based VS Code position. */
export function servicePositionToVscode(
  text: string,
  position: ServicePosition
): VscodePosition {
  const lines = lineTexts(text);
  const line = Math.max(0, Math.min(lines.length - 1, position.line - 1));
  return {
    line,
    character: utf16Column(lines[line] ?? "", position.col),
  };
}

/** Convert a 0-based VS Code position to a 1-based scalar service position. */
export function vscodePositionToService(
  text: string,
  position: VscodePosition
): ServicePosition {
  const lines = lineTexts(text);
  const line = Math.max(0, Math.min(lines.length - 1, position.line));
  const prefix = (lines[line] ?? "").slice(0, position.character);
  return { line: line + 1, col: Array.from(prefix).length + 1 };
}

/** The end position of a core span, walking its scalar length across lines. */
export function spanEnd(text: string, span: Span): ServicePosition {
  const lines = lineTexts(text);
  let line = Math.max(1, span.line);
  let col = Math.max(1, span.col);
  let remaining = span.len;
  while (remaining > 0) {
    const content = lines[line - 1] ?? "";
    const length = Array.from(content).length;
    const available = Math.max(0, length - (col - 1));
    if (remaining <= available) {
      col += remaining;
      remaining = 0;
    } else {
      remaining -= available;
      if (remaining === 0) {
        break;
      }
      remaining -= 1;
      line += 1;
      col = 1;
    }
  }
  return { line, col };
}

/** A core span as a VS Code range. */
export function spanToRange(text: string, span: Span): VscodeRange {
  return {
    start: servicePositionToVscode(text, {
      line: span.line,
      col: span.col,
    }),
    end: servicePositionToVscode(text, spanEnd(text, span)),
  };
}

/** Convert a VS Code range to service positions. */
export function vscodeRangeToService(
  text: string,
  range: VscodeRange
): { start: ServicePosition; end: ServicePosition } {
  return {
    start: vscodePositionToService(text, range.start),
    end: vscodePositionToService(text, range.end),
  };
}
