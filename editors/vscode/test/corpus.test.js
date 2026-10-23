// The standalone WASM front end against `fixtures/lang-corpus`.
//
// This is the Node half of the parity gate: the Rust service and the embedded
// LSP run the same files through `crates/impresario-language/tests/corpus.rs`
// and `crates/impresario-master/tests/lsp_corpus.rs`.
"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

const { LanguageService } = require("../out/language-service");

const root = path.resolve(__dirname, "../../../fixtures/lang-corpus");
const filesRoot = path.join(root, "files");

function collect(dir, files = []) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      collect(full, files);
    } else if (/\.(scene|theatre|play)$/.test(entry.name)) {
      files.push(full);
    }
  }
  return files;
}

function sources() {
  return collect(filesRoot)
    .sort()
    .map((file) => ({
      path: path.relative(filesRoot, file).split(path.sep).join("/"),
      text: fs.readFileSync(file, "utf8"),
    }));
}

function readJson(name) {
  return JSON.parse(fs.readFileSync(path.join(root, name), "utf8"));
}

async function service() {
  return LanguageService.load(
    path.resolve(__dirname, "../media/impresario_language.wasm")
  );
}

function normalized(diagnostics) {
  return diagnostics.map((diagnostic) => ({
    code: diagnostic.code,
    severity: diagnostic.severity,
    line: diagnostic.span.line,
    col: diagnostic.span.col,
    message: diagnostic.message,
    related: (diagnostic.related || []).map((related) => ({
      line: related.span.line,
      col: related.span.col,
      message: related.message,
    })),
  }));
}

test("standalone diagnostics match the shared corpus", async () => {
  const files = sources();
  const language = await service();
  const result = language.analyze({
    sources: files,
    devicesYaml: fs.readFileSync(path.join(root, "devices.config"), "utf8"),
    devicesPath: "impresario.devices.config",
    appsYaml: fs.readFileSync(path.join(root, "apps.config"), "utf8"),
    appsPath: "impresario.apps.config",
    root,
  });
  const expected = readJson("expected-diagnostics.json");
  const actual = {};
  for (const file of files) {
    actual[file.path] = normalized(result.diagnostics[file.path] || []);
  }
  assert.deepEqual(actual, expected);
  assert.deepEqual(
    Object.keys(actual).sort(),
    Object.keys(expected).sort(),
    "expectations must cover every corpus file"
  );
});

test("standalone formatting matches the shared corpus", async () => {
  const language = await service();
  const files = sources();
  language.analyze({ sources: files, root });
  for (const file of files) {
    const edit = language.format(file.path);
    const formatted = edit ? edit.new_text : file.text;
    const expected = fs.readFileSync(
      path.join(root, "expected-format", ...file.path.split("/")),
      "utf8"
    );
    assert.equal(formatted, expected, `format differs for ${file.path}`);
  }
});

test("standalone completion probes match the shared corpus", async () => {
  const language = await service();
  language.analyze({ sources: sources(), root });
  for (const probe of readJson("completion-probes.json")) {
    const labels = language
      .completion(probe.path, { line: probe.line, col: probe.col })
      .map((item) => item.label);
    for (const label of probe.contains) {
      assert.ok(
        labels.includes(label),
        `${probe.path}:${probe.line}:${probe.col} lacks ${JSON.stringify(
          label
        )}; got ${JSON.stringify(labels)}`
      );
    }
  }
});

function symbolNames(symbols, names = []) {
  for (const symbol of symbols) {
    names.push(symbol.name);
    symbolNames(symbol.children || [], names);
  }
  return names;
}

test("standalone feature probes match the shared corpus", async () => {
  const language = await service();
  language.analyze({ sources: sources(), root });
  for (const probe of readJson("feature-probes.json")) {
    const position = probe.line
      ? { line: probe.line, col: probe.col }
      : undefined;
    switch (probe.op) {
      case "hover": {
        const hover = language.hover(probe.path, position);
        assert.ok(hover, `${probe.path} has no hover`);
        for (const needle of probe.contains) {
          assert.ok(
            hover.markdown.includes(needle),
            `${probe.path} hover lacks ${JSON.stringify(needle)}: ${
              hover.markdown
            }`
          );
        }
        break;
      }
      case "definition": {
        const spans = language.definition(probe.path, position);
        assert.equal(spans.length, probe.expect.length);
        for (const [span, expected] of spans.map((span, index) => [
          span,
          probe.expect[index],
        ])) {
          assert.equal(span.file, expected.path);
          assert.equal(span.line, expected.line);
          assert.equal(span.col, expected.col);
        }
        break;
      }
      case "references": {
        const spans = language.references(probe.path, position, true);
        assert.ok(
          spans.length >= probe.min,
          `${probe.path} references: ${JSON.stringify(spans)}`
        );
        for (const expected of probe.expect_contains) {
          assert.ok(
            spans.some(
              (span) =>
                span.file === expected.path &&
                span.line === expected.line &&
                span.col === expected.col
            ),
            `${probe.path} references lack ${JSON.stringify(expected)}`
          );
        }
        break;
      }
      case "prepareRename": {
        const prepared = language.prepareRename(probe.path, position);
        assert.ok(prepared);
        assert.equal(prepared.placeholder, probe.placeholder);
        break;
      }
      case "rename": {
        const edit = language.rename(probe.path, position, probe.new_name);
        assert.deepEqual(
          Object.keys(edit.changes).sort(),
          [...probe.changed_files].sort()
        );
        const edits = Object.values(edit.changes).flat();
        assert.equal(edits.length, probe.expected_edits);
        assert.ok(edits.every((change) => change.new_text === probe.new_name));
        break;
      }
      case "documentSymbols": {
        const names = symbolNames(language.documentSymbols(probe.path));
        for (const expected of probe.contains) {
          assert.ok(names.includes(expected), `${probe.path}: ${names}`);
        }
        break;
      }
      case "workspaceSymbols": {
        const names = language
          .workspaceSymbols(probe.query)
          .map((symbol) => symbol.name);
        for (const expected of probe.contains) {
          assert.ok(names.includes(expected), `${probe.query}: ${names}`);
        }
        break;
      }
      case "folding": {
        const ranges = language.folding(probe.path);
        assert.ok(ranges.length >= probe.min, JSON.stringify(ranges));
        break;
      }
      case "semanticTokens": {
        const tokens = language.semanticTokens(probe.path);
        assert.ok(tokens.length >= probe.min, JSON.stringify(tokens));
        for (const expected of probe.contains_types) {
          assert.ok(
            tokens.some((token) => token.token_type === expected),
            JSON.stringify(tokens)
          );
        }
        break;
      }
      case "signatureHelp": {
        const help = language.signatureHelp(probe.path, position);
        assert.ok(help);
        assert.ok(
          help.signatures.some((signature) =>
            signature.label.includes(probe.label_contains)
          ),
          JSON.stringify(help)
        );
        assert.equal(help.active_parameter, probe.active_parameter);
        break;
      }
      case "codeActions": {
        const titles = language
          .codeActions(probe.path, probe.start, probe.end)
          .map((action) => action.title);
        for (const expected of probe.contains_titles) {
          assert.ok(titles.includes(expected), JSON.stringify(titles));
        }
        break;
      }
      case "inlayHints": {
        const hints = language.inlayHints(
          probe.path,
          probe.line,
          probe.line
        );
        for (const expected of probe.contains) {
          assert.ok(
            hints.some((hint) => hint.label.includes(expected)),
            JSON.stringify(hints)
          );
        }
        break;
      }
      case "codeLenses": {
        const titles = language
          .codeLenses(probe.path)
          .map((lens) => lens.command.title);
        for (const expected of probe.contains) {
          assert.ok(
            titles.some((title) => title.includes(expected)),
            JSON.stringify(titles)
          );
        }
        break;
      }
      case "rangeFormatting": {
        const edits = language.rangeFormatting(
          probe.path,
          probe.start,
          probe.end
        );
        assert.ok(
          edits.some((edit) => edit.new_text.includes(probe.new_text_contains)),
          JSON.stringify(edits)
        );
        break;
      }
      case "onTypeFormatting": {
        const edits = language.onTypeFormatting(
          probe.path,
          { line: probe.line, col: probe.col },
          probe.ch
        );
        assert.ok(
          edits.some((edit) => edit.new_text.includes(probe.new_text_contains)),
          JSON.stringify(edits)
        );
        break;
      }
      default:
        assert.fail(`unknown feature probe ${probe.op}`);
    }
  }
});
