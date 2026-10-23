// Bundle the strict TypeScript sources into the extension entry point. The
// wasm service is shipped separately in media/.
"use strict";

const esbuild = require("esbuild");
const path = require("path");

const root = __dirname;

async function main() {
  await esbuild.build({
    entryPoints: [path.join(root, "src", "extension.ts")],
    outfile: path.join(root, "out", "extension.js"),
    bundle: true,
    platform: "node",
    format: "cjs",
    target: "node18",
    external: ["vscode"],
    sourcemap: true,
    logLevel: "info",
  });
  await esbuild.build({
    entryPoints: [path.join(root, "src", "language-service.ts")],
    outfile: path.join(root, "out", "language-service.js"),
    bundle: true,
    platform: "node",
    format: "cjs",
    target: "node18",
    sourcemap: true,
    logLevel: "info",
  });
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
