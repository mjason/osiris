import * as fs from "node:fs";
import * as path from "node:path";
import * as vscode from "vscode";
import { registerGeneratedSourceNavigation } from "./sourcemap";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions
} from "vscode-languageclient/node";
import {
  EmbeddedRegionResponse,
  registerEmbeddedLanguageSupport
} from "./embedded";

let client: LanguageClient | undefined;
const MINIMUM_SERVER_VERSION = [0, 3, 0] as const;

function supportedServerVersion(version: string | undefined): boolean {
  if (version === undefined) {
    return false;
  }
  const match = /^(\d+)\.(\d+)\.(\d+)(?:[-+].*)?$/.exec(version);
  if (match === null) {
    return false;
  }
  const actual = [
    Number(match[1] ?? -1),
    Number(match[2] ?? -1),
    Number(match[3] ?? -1)
  ] as const;
  return actual[0] > MINIMUM_SERVER_VERSION[0]
    || (actual[0] === MINIMUM_SERVER_VERSION[0]
      && (actual[1] > MINIMUM_SERVER_VERSION[1]
        || (actual[1] === MINIMUM_SERVER_VERSION[1]
          && actual[2] >= MINIMUM_SERVER_VERSION[2])));
}

class StandardSourceProvider implements vscode.TextDocumentContentProvider {
  async provideTextDocumentContent(uri: vscode.Uri): Promise<string> {
    if (client === undefined) {
      throw new Error("Osiris language server is not running");
    }
    const result = await client.sendRequest<{ text: string }>(
      "osiris/standardSource",
      { uri: uri.toString() }
    );
    return result.text;
  }
}

/// Locates the `osr` the workspace itself installs.
///
/// Osiris projects declare `osiris-lang` as a uv dependency, so the compiler
/// that matches the project is the one in its environment. Preferring it over
/// whatever `osr` happens to be on PATH keeps the language server in step with
/// `osr check` and with a local or pinned compiler build.
function workspaceServerPath(): string | undefined {
  // Environment layout differs by platform, and this extension is
  // `workspace`-kind, so these always describe the machine holding the
  // environment rather than the machine showing the window.
  const executable = process.platform === "win32" ? "osr.exe" : "osr";
  const binary = process.platform === "win32" ? "Scripts" : "bin";
  const roots: string[] = [];
  const active = process.env.VIRTUAL_ENV;
  if (active !== undefined && active.length > 0) {
    roots.push(active);
  }
  // `.venv` is the conventional project environment directory; an environment
  // anywhere else announces itself through VIRTUAL_ENV above.
  for (const folder of vscode.workspace.workspaceFolders ?? []) {
    if (folder.uri.scheme === "file") {
      roots.push(path.join(folder.uri.fsPath, ".venv"));
    }
  }
  // Windows has no execute permission bit, so only POSIX asks for one; a stale
  // directory entry or unreadable path falls through to PATH either way rather
  // than failing the server launch.
  const access =
    process.platform === "win32" ? fs.constants.F_OK : fs.constants.X_OK;
  for (const root of roots) {
    const candidate = path.join(root, binary, executable);
    try {
      fs.accessSync(candidate, access);
      if (fs.statSync(candidate).isFile()) {
        return candidate;
      }
    } catch {
      // Not every workspace folder has an environment; keep looking.
    }
  }
  return undefined;
}

function createClient(): LanguageClient {
  const configuration = vscode.workspace.getConfiguration("osiris");
  const configured = configuration.inspect<string>("server.path");
  const explicit =
    configured?.workspaceFolderValue
    ?? configured?.workspaceValue
    ?? configured?.globalValue;
  // An explicit setting always wins; otherwise prefer the workspace
  // environment and fall back to PATH.
  const command = explicit ?? workspaceServerPath() ?? "osr";
  const args = configuration.get<string[]>("server.arguments", ["lsp"]);
  // The server reads its level from the environment, so the setting has to
  // reach the process rather than the protocol; `debug` records every request
  // and whether it found anything.
  const log = configuration.get<string>("server.log", "info");
  const configuredLocale = configuration.get<string>("displayLocale", "").trim();
  const siteRoots = configuration.get<string[]>("server.siteRoots", []);

  const options = { env: { ...process.env, OSIRIS_LSP_LOG: log } };
  const serverOptions: ServerOptions = {
    run: { command, args, options },
    debug: { command, args, options }
  };
  const clientOptions: LanguageClientOptions = {
    documentSelector: [
      { language: "osiris", scheme: "file" },
      { language: "osiris", scheme: "untitled" }
    ],
    synchronize: {
      configurationSection: "osiris",
      fileEvents: vscode.workspace.createFileSystemWatcher("**/*.{osr,osri}")
    },
    initializationOptions: {
      displayLocale: configuredLocale || vscode.env.language,
      siteRoots
    }
  };

  return new LanguageClient(
    "osiris",
    "Osiris Language Server",
    serverOptions,
    clientOptions
  );
}

async function startClient(): Promise<void> {
  client = createClient();
  try {
    await client.start();
    const version = client.initializeResult?.serverInfo?.version;
    if (!supportedServerVersion(version)) {
      await client.stop();
      client = undefined;
      const actual = version === undefined ? "an unknown version" : `version ${version}`;
      void vscode.window.showErrorMessage(
        `Osiris language support requires osr 0.3.0 or newer, but ${actual} was found. Upgrade osiris-lang or configure osiris.server.path, then restart the language server.`
      );
    }
  } catch (error: unknown) {
    client = undefined;
    const message = error instanceof Error ? error.message : String(error);
    void vscode.window.showErrorMessage(
      `Unable to start osr lsp: ${message}. Install osiris-lang or configure osiris.server.path.`
    );
  }
}

async function restartClient(): Promise<void> {
  if (client !== undefined) {
    await client.stop();
    client = undefined;
  }
  await startClient();
}

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  context.subscriptions.push(
    vscode.workspace.registerTextDocumentContentProvider(
      "osiris-stdlib",
      new StandardSourceProvider()
    ),
    vscode.commands.registerCommand(
      "osiris.restartLanguageServer",
      restartClient
    )
  );
  registerGeneratedSourceNavigation(context);
  await startClient();
  registerEmbeddedLanguageSupport(context, async (uri) => {
    if (client === undefined) {
      throw new Error("Osiris language server is not running");
    }
    return client.sendRequest<EmbeddedRegionResponse>("osiris/embeddedRegions", { uri });
  });
}

export async function deactivate(): Promise<void> {
  if (client !== undefined) {
    await client.stop();
    client = undefined;
  }
}
