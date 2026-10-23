"use strict";

const path = require("node:path");
const Mocha = require("mocha");

function run() {
  const mocha = new Mocha({
    ui: "bdd",
    timeout: 60000,
    color: true,
  });
  mocha.addFile(path.resolve(__dirname, "extension.test.js"));
  return new Promise((resolve, reject) => {
    mocha.run((failures) => {
      if (failures > 0) {
        reject(new Error(`${failures} extension host test(s) failed`));
      } else {
        resolve();
      }
    });
  });
}

module.exports = { run };
