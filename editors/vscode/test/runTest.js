// Launch the real VS Code extension host for the standalone provider tests.
//
// The corpus is copied into a throw-away workspace so the test can exercise
// workspace indexing, diagnostics and formatting without mutating fixtures.
"use strict";

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { runTests } = require("@vscode/test-electron");

async function main() {
  const extensionDevelopmentPath = path.resolve(__dirname, "..");
  const extensionTestsPath = path.resolve(__dirname, "suite", "index.js");
  const repositoryRoot = path.resolve(extensionDevelopmentPath, "..", "..");
  const corpus = path.join(repositoryRoot, "fixtures", "lang-corpus");
  const workspace = fs.mkdtempSync(
    path.join(os.tmpdir(), "impresario-vscode-")
  );
  try {
    fs.cpSync(path.join(corpus, "files"), workspace, { recursive: true });
    for (const name of ["devices.config", "apps.config"]) {
      fs.copyFileSync(
        path.join(corpus, name),
        path.join(workspace, name === "devices.config"
          ? "impresario.devices.config"
          : "impresario.apps.config")
      );
    }
    fs.mkdirSync(path.join(workspace, ".vscode"), { recursive: true });
    fs.writeFileSync(
      path.join(workspace, ".vscode", "settings.json"),
      JSON.stringify({ "impresario.mode": "standalone" }, null, 2)
    );

    await runTests({
      version: process.env.VSCODE_TEST_VERSION || "stable",
      extensionDevelopmentPath,
      extensionTestsPath,
      launchArgs: [
        workspace,
        "--disable-extensions",
        "--disable-workspace-trust",
        "--disable-gpu",
      ],
    });
  } finally {
    fs.rmSync(workspace, { recursive: true, force: true });
  }
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
