// Responsiveness gate for the standalone workspace index (TASK-2 9.2).
//
// The extension debounces edits and re-analyzes in the WASM service. This test
// exercises the same path with a 1,000+ file workspace and a change overlay.
"use strict";

const assert = require("node:assert/strict");
const path = require("node:path");
const test = require("node:test");

const { LanguageService } = require("../out/language-service");

function workspace() {
  const sources = [];
  for (let index = 0; index < 1050; index += 1) {
    sources.push({
      path: `scenes/Scene${index}.scene`,
      text:
        `Scene: Scene${index}\n` +
        `Platform: cross-platform\n\n` +
        `| Piece | web | android |\n` +
        `|---|---|---|\n` +
        `| Button${index} | #button${index} | id:button${index} |\n`,
    });
  }
  for (let index = 0; index < 60; index += 1) {
    sources.push({
      path: `theatres/Theatre${index}.theatre`,
      text:
        `Theatre: Theatre${index}\n\n` +
        `Stage: Stage${index}(value = 'x')\n` +
        `    Scene: Scene${index}\n` +
        `        Check: Button${index} is visible\n`,
    });
  }
  for (let index = 0; index < 60; index += 1) {
    sources.push({
      path: `plays/Play${index}.play`,
      text:
        `Play: Play${index}\n\n` +
        `Act: Act${index}\n` +
        `    Perform: Stage${index}()\n`,
    });
  }
  return sources;
}

async function service() {
  return LanguageService.load(
    path.resolve(__dirname, "../media/impresario_language.wasm")
  );
}

test("standalone stays responsive on a 1,000+ file workspace", async () => {
  const language = await service();
  const sources = workspace();
  const devices = "devices:\n  - name: web1\n    type: web\n    ip: 127.0.0.1\n";

  const first = performance.now();
  language.analyze({ sources, devicesYaml: devices });
  const initial = performance.now() - first;

  const changed = sources.map((source) =>
    source.path === "scenes/Scene0.scene"
      ? { ...source, text: source.text.replace("#button0", "#changed") }
      : source
  );
  const second = performance.now();
  language.analyze({ sources: changed, devicesYaml: devices });
  const incremental = performance.now() - second;

  const third = performance.now();
  const labels = language
    .completion("plays/Play0.play", { line: 4, col: 16 })
    .map((item) => item.label);
  const completion = performance.now() - third;

  assert.ok(labels.includes("Stage0"), JSON.stringify(labels));
  assert.ok(
    initial < 5000,
    `initial analysis of ${sources.length} files took ${initial.toFixed(0)}ms`
  );
  assert.ok(
    incremental < 5000,
    `re-analysis after an edit took ${incremental.toFixed(0)}ms`
  );
  assert.ok(
    completion < 500,
    `completion after indexing took ${completion.toFixed(0)}ms`
  );
  console.log(
    `benchmark: ${sources.length} files, initial ${initial.toFixed(
      0
    )}ms, re-analysis ${incremental.toFixed(0)}ms, completion ${completion.toFixed(
      0
    )}ms`
  );
});
