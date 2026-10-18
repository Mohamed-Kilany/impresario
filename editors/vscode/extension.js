// Minimal VS Code client: launch `impresario lsp` on stdio for .play/.scene
// buffers and connect the language client to it.
"use strict";

const vscode = require("vscode");
const { LanguageClient, TransportKind } = require("vscode-languageclient/node");

/** @type {import("vscode-languageclient/node").LanguageClient | undefined} */
let client;

function workspaceRoot() {
  const configured = vscode.workspace.getConfiguration("impresario").get("workspace");
  if (configured) {
    return configured;
  }
  const folders = vscode.workspace.workspaceFolders;
  return folders && folders.length > 0 ? folders[0].uri.fsPath : undefined;
}

async function activate() {
  const config = vscode.workspace.getConfiguration("impresario");
  const root = workspaceRoot();
  const args = ["lsp"];
  if (root) {
    args.push("--workspace", root);
  }
  const configPath = config.get("config");
  const devicesPath = config.get("devices");
  if (configPath) {
    args.push("--config", configPath);
  }
  if (devicesPath) {
    args.push("--devices-config", devicesPath);
  }

  const serverOptions = {
    command: config.get("serverPath") || "impresario",
    args,
    transport: TransportKind.stdio,
  };
  const clientOptions = {
    documentSelector: [
      { language: "impresario-play" },
      { language: "impresario-scene" },
    ],
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

async function deactivate() {
  if (client) {
    await client.stop();
    client = undefined;
  }
}

module.exports = { activate, deactivate };
