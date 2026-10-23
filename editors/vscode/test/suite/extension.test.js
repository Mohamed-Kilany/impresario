// End-to-end checks against a real VS Code extension host in standalone mode.
"use strict";

const assert = require("node:assert/strict");
const path = require("node:path");
const vscode = require("vscode");

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

async function waitFor(predicate, message, timeout = 30000) {
  const deadline = Date.now() + timeout;
  let lastError;
  while (Date.now() < deadline) {
    try {
      const value = await predicate();
      if (value) {
        return value;
      }
    } catch (error) {
      lastError = error;
    }
    await delay(100);
  }
  throw new Error(
    `${message}${lastError ? `: ${lastError.message}` : ""}`
  );
}

function labelOf(item) {
  return typeof item.label === "string" ? item.label : item.label.label;
}

async function completionLabels(uri, position) {
  const result = await vscode.commands.executeCommand(
    "vscode.executeCompletionItemProvider",
    uri,
    position
  );
  const items = Array.isArray(result) ? result : result.items;
  return items.map(labelOf);
}

async function open(name) {
  const folders = vscode.workspace.workspaceFolders;
  assert.ok(folders && folders.length > 0, "test workspace is open");
  const uri = vscode.Uri.joinPath(folders[0].uri, ...name.split("/"));
  const document = await vscode.workspace.openTextDocument(uri);
  await vscode.window.showTextDocument(document);
  return document;
}

describe("Impresario standalone extension", function () {
  this.timeout(60000);
  let extension;

  before(async function () {
    extension = vscode.extensions.all.find(
      (candidate) => candidate.packageJSON.name === "impresario"
    );
    assert.ok(extension, "extension is discovered");
    await extension.activate();
    assert.equal(vscode.workspace.getConfiguration("impresario").get("mode"), "standalone");
  });

  it("publishes diagnostics for .play files", async function () {
    const document = await open("plays/broken.play");
    const diagnostics = await waitFor(
      () => {
        const list = vscode.languages.getDiagnostics(document.uri);
        return list.length >= 4 ? list : undefined;
      },
      "diagnostics did not arrive"
    );
    const codes = diagnostics.map((diagnostic) => diagnostic.code);
    for (const code of [
      "IMP-DSL-046",
      "IMP-DSL-031",
      "IMP-DSL-037",
      "IMP-DSL-032",
    ]) {
      assert.ok(codes.includes(code), `missing ${code}: ${codes.join(", ")}`);
    }
    assert.ok(
      diagnostics.every((diagnostic) => diagnostic.source === "impresario"),
      "diagnostics come from the impresario source"
    );
    assert.ok(
      diagnostics.every((diagnostic) => !diagnostic.relatedInformation),
      "the broken corpus file has no related information"
    );
  });

  it("provides stage completion in a .play file", async function () {
    const document = await open("plays/login.play");
    const labels = await waitFor(
      async () => {
        const labels = await completionLabels(
          document.uri,
          new vscode.Position(7, 18)
        );
        return labels.includes("Fill") ? labels : undefined;
      },
      "stage completion did not arrive"
    );
    assert.ok(labels.includes("Fill"));
    const parameterLabels = await completionLabels(
      document.uri,
      new vscode.Position(7, 23)
    );
    assert.ok(parameterLabels.includes("user = "));
    assert.ok(parameterLabels.includes("password = "));
  });

  it("provides hover and definitions across the workspace", async function () {
    const document = await open("plays/login.play");
    const hovers = await waitFor(
      async () => {
        const hovers = await vscode.commands.executeCommand(
          "vscode.executeHoverProvider",
          document.uri,
          new vscode.Position(7, 18)
        );
        return hovers && hovers.length > 0 ? hovers : undefined;
      },
      "hover did not arrive"
    );
    const markdown = hovers
      .flatMap((hover) => hover.contents)
      .map((content) => (typeof content === "string" ? content : content.value))
      .join("\n");
    for (const needle of ["**Fill**", "stage in theatre `Auth`", "4 references"]) {
      assert.ok(
        markdown.includes(needle),
        `hover lacks ${JSON.stringify(needle)}: ${markdown}`
      );
    }

    const definition = await vscode.commands.executeCommand(
      "vscode.executeDefinitionProvider",
      document.uri,
      new vscode.Position(7, 18)
    );
    const locations = Array.isArray(definition) ? definition : [definition];
    assert.equal(locations.length, 1);
    assert.ok(locations[0].uri.fsPath.endsWith(path.join("theatres", "auth.theatre")));
    assert.equal(locations[0].range.start.line, 2);
    assert.equal(locations[0].range.start.character, 7);
  });

  it("formats whole documents, ranges and on-type edits", async function () {
    const scene = await open("scenes/LoginScreen.scene");
    const range = new vscode.Range(3, 0, 5, 20);
    const rangeEdits = await vscode.commands.executeCommand(
      "vscode.executeFormatRangeProvider",
      scene.uri,
      range
    );
    assert.ok(
      rangeEdits && rangeEdits.length > 0,
      "range formatting returned edits"
    );

    const onType = await vscode.commands.executeCommand(
      "vscode.executeFormatOnTypeProvider",
      scene.uri,
      new vscode.Position(3, 0),
      "|"
    );
    assert.ok(onType && onType.length > 0, "on-type formatting returned edits");
    const typed = new vscode.WorkspaceEdit();
    for (const edit of onType) {
      typed.replace(scene.uri, edit.range, edit.newText);
    }
    assert.ok(await vscode.workspace.applyEdit(typed), "on-type edit applies");
    assert.ok(
      scene.getText().includes("| UsernameInput |"),
      scene.getText()
    );

    const play = await open("plays/examples.play");
    const edits = await vscode.commands.executeCommand(
      "vscode.executeFormatDocumentProvider",
      play.uri
    );
    assert.ok(edits && edits.length > 0, "document formatting returned edits");
    const formatted = new vscode.WorkspaceEdit();
    for (const edit of edits) {
      formatted.replace(play.uri, edit.range, edit.newText);
    }
    assert.ok(
      await vscode.workspace.applyEdit(formatted),
      "document formatting edit applies"
    );
    assert.ok(
      play.getText().includes("| user  | password |"),
      play.getText()
    );
  });

  it("provides .theatre completion, symbols and code actions", async function () {
    const theatre = await open("theatres/auth.theatre");
    assert.equal(theatre.languageId, "impresario-theatre");
    const labels = await waitFor(
      async () => {
        const labels = await completionLabels(
          theatre.uri,
          new vscode.Position(4, 11)
        );
        return labels.includes("LoginScreen") ? labels : undefined;
      },
      "scene completion did not arrive"
    );
    assert.ok(labels.includes("LoginScreen"));

    const symbols = await vscode.commands.executeCommand(
      "vscode.executeDocumentSymbolProvider",
      theatre.uri
    );
    assert.ok(symbols && symbols.length > 0, "document symbols exist");
    const names = [];
    const collect = (symbols) => {
      for (const symbol of symbols) {
        names.push(symbol.name);
        if (symbol.children) {
          collect(symbol.children);
        }
      }
    };
    collect(symbols);
    assert.ok(names.includes("Auth"), names.join(", "));
    assert.ok(names.includes("Fill"), names.join(", "));

    const broken = await open("plays/broken.play");
    const actions = await vscode.commands.executeCommand(
      "vscode.executeCodeActionProvider",
      broken.uri,
      new vscode.Range(6, 13, 6, 21)
    );
    assert.ok(
      actions.some((action) => action.title === "Use the suggested spelling"),
      actions.map((action) => action.title).join(", ")
    );
  });
});
