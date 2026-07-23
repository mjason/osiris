import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

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
