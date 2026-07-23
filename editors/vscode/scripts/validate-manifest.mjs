import { readFileSync } from "node:fs";

function readJson(path) {
  return JSON.parse(readFileSync(new URL(path, import.meta.url), "utf8"));
}

const manifest = readJson("../package.json");
const grammar = readJson("../syntaxes/osiris.tmLanguage.json");
readJson("../language-configuration.json");

if (manifest.name !== "osiris" || manifest.contributes.languages[0].id !== "osiris") {
  throw new Error("The extension and language IDs must remain `osiris`");
}
if (grammar.scopeName !== manifest.contributes.grammars[0].scopeName) {
  throw new Error("The TextMate grammar scope does not match package.json");
}
if (!manifest.contributes.languages[0].extensions.includes(".osr")) {
  throw new Error("The extension must register .osr files");
}

console.log(`validated Osiris VS Code extension ${manifest.version}`);
