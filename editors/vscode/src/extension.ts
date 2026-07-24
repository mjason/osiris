import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions
} from "vscode-languageclient/node";

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

function createClient(): LanguageClient {
  const configuration = vscode.workspace.getConfiguration("osiris");
  const command = configuration.get<string>("server.path", "osr");
  const args = configuration.get<string[]>("server.arguments", ["lsp"]);
  const configuredLocale = configuration.get<string>("displayLocale", "").trim();
  const siteRoots = configuration.get<string[]>("server.siteRoots", []);

  const serverOptions: ServerOptions = {
    run: { command, args },
    debug: { command, args }
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
  await startClient();
}

export async function deactivate(): Promise<void> {
  if (client !== undefined) {
    await client.stop();
    client = undefined;
  }
}
