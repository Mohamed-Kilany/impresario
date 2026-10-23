// Build the standalone language service and copy it into the extension.
//
// The crate uses a hand-written JSON ABI instead of wasm-bindgen, so the only
// tool required is a Rust toolchain with the wasm32-unknown-unknown target:
//
//   rustup target add wasm32-unknown-unknown
//   npm run build:wasm
"use strict";

const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

const extensionRoot = path.resolve(__dirname, "..");
const repositoryRoot = path.resolve(extensionRoot, "..", "..");
const cargo = process.env.CARGO || "cargo";
const args = [
  "build",
  "--release",
  "--target",
  "wasm32-unknown-unknown",
  "--package",
  "impresario-language-wasm",
];

const result = spawnSync(cargo, args, {
  cwd: repositoryRoot,
  stdio: "inherit",
  env: process.env,
});
if (result.error) {
  console.error(
    `failed to run ${cargo}: ${result.error.message}\n` +
      "install Rust and run `rustup target add wasm32-unknown-unknown`"
  );
  process.exit(1);
}
if (result.status !== 0) {
  process.exit(result.status ?? 1);
}

const built = path.join(
  repositoryRoot,
  "target",
  "wasm32-unknown-unknown",
  "release",
  "impresario_language_wasm.wasm"
);
if (!fs.existsSync(built)) {
  console.error(`cargo did not produce ${built}`);
  process.exit(1);
}
const media = path.join(extensionRoot, "media");
fs.mkdirSync(media, { recursive: true });
const destination = path.join(media, "impresario_language.wasm");
fs.copyFileSync(built, destination);
const size = fs.statSync(destination).size;
console.log(`copied ${built} -> ${destination} (${size} bytes)`);
