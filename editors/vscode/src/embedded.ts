import * as vscode from "vscode";

export interface EmbeddedRegionResponse {
  schema: "osiris.embedded-regions/v1";
  documentVersion: number;
  regions: EmbeddedRegion[];
}

interface EmbeddedLineMap {
  embeddedLine: number;
  hostLine: number;
  hostCharacter: number;
}

interface EmbeddedRegion {
  id: string;
  language: string;
  label: string;
  hostSpan: { start: number; end: number };
  bodySpan: { start: number; end: number };
  text: string;
  lineMap: EmbeddedLineMap[];
}

interface RegionDocument {
  host: vscode.TextDocument;
  region: EmbeddedRegion;
  uri: vscode.Uri;
}

type RegionFetcher = (uri: string) => Promise<EmbeddedRegionResponse>;

const LANGUAGE_IDS: Readonly<Record<string, string>> = {
  python: "python",
  markdown: "markdown",
  json: "json",
  html: "html",
  css: "css",
  javascript: "javascript",
  typescript: "typescript",
  sql: "sql",
  toml: "toml",
  yaml: "yaml"
};

const EXTENSIONS: Readonly<Record<string, string>> = {
  python: "py",
  markdown: "md",
  json: "json",
  html: "html",
  css: "css",
  javascript: "js",
  typescript: "ts",
  sql: "sql",
  toml: "toml",
  yaml: "yaml"
};

class EmbeddedContentProvider implements vscode.TextDocumentContentProvider {
  private readonly regions = new Map<string, RegionDocument>();
  private readonly changed = new vscode.EventEmitter<vscode.Uri>();
  readonly onDidChange = this.changed.event;

  set(entry: RegionDocument): void {
    this.regions.set(entry.uri.toString(), entry);
    this.changed.fire(entry.uri);
  }

  get(uri: vscode.Uri): RegionDocument | undefined {
    return this.regions.get(uri.toString());
  }

  provideTextDocumentContent(uri: vscode.Uri): string {
    return this.regions.get(uri.toString())?.region.text ?? "";
  }

  dispose(): void {
    this.changed.dispose();
    this.regions.clear();
  }
}

export function registerEmbeddedLanguageSupport(
  context: vscode.ExtensionContext,
  fetchRegions: RegionFetcher
): void {
  const content = new EmbeddedContentProvider();
  const diagnostics = vscode.languages.createDiagnosticCollection("osiris-embedded");
  const diagnosticsByHost = new Map<string, Map<string, vscode.Diagnostic[]>>();
  const selector: vscode.DocumentSelector = [{ language: "osiris" }];

  async function regionsFor(host: vscode.TextDocument): Promise<RegionDocument[]> {
    const response = await fetchRegions(host.uri.toString());
    if (response.documentVersion !== host.version) {
      return [];
    }
    return response.regions
      .filter((region) => region.language !== "osiris")
      .map((region) => {
        const language = configuredLanguageId(region.language);
        const extension = EXTENSIONS[region.language] ?? region.language;
        const uri = vscode.Uri.from({
          scheme: "osiris-embedded",
          path: `/${encodeURIComponent(host.uri.toString())}/${encodeURIComponent(region.label)}.${extension}`,
          query: `version=${host.version}&id=${encodeURIComponent(region.id)}&language=${encodeURIComponent(language)}`
        });
        return { host, region, uri };
      });
  }

  async function atPosition(
    host: vscode.TextDocument,
    position: vscode.Position
  ): Promise<{ entry: RegionDocument; position: vscode.Position } | undefined> {
    let regions: RegionDocument[];
    try {
      regions = await regionsFor(host);
    } catch {
      return undefined;
    }
    for (const entry of regions) {
      const mapped = toEmbeddedPosition(entry, position);
      if (mapped !== undefined) {
        await openRegion(entry, content);
        return { entry, position: mapped };
      }
    }
    return undefined;
  }

  async function refreshHost(host: vscode.TextDocument): Promise<void> {
    if (host.languageId !== "osiris") return;
    try {
      await Promise.all((await regionsFor(host)).map((entry) => openRegion(entry, content)));
    } catch {
      // The compiler server may still be starting. Host-language features stay available.
    }
  }

  context.subscriptions.push(
    content,
    diagnostics,
    vscode.workspace.registerTextDocumentContentProvider("osiris-embedded", content),
    vscode.languages.registerCompletionItemProvider(selector, {
      async provideCompletionItems(document, position, _token, context) {
        const target = await atPosition(document, position);
        if (target === undefined) return undefined;
        return vscode.commands.executeCommand<vscode.CompletionList>(
          "vscode.executeCompletionItemProvider",
          target.entry.uri,
          target.position,
          context.triggerCharacter
        );
      }
    }, ".", "\"", "'", "/", ":"),
    vscode.languages.registerHoverProvider(selector, {
      async provideHover(document, position) {
        const target = await atPosition(document, position);
        if (target === undefined) return undefined;
        const values = await vscode.commands.executeCommand<vscode.Hover[]>(
          "vscode.executeHoverProvider",
          target.entry.uri,
          target.position
        );
        return values?.[0];
      }
    }),
    vscode.languages.registerSignatureHelpProvider(selector, {
      async provideSignatureHelp(document, position, _token, context) {
        const target = await atPosition(document, position);
        if (target === undefined) return undefined;
        return vscode.commands.executeCommand<vscode.SignatureHelp>(
          "vscode.executeSignatureHelpProvider",
          target.entry.uri,
          target.position,
          context.triggerCharacter
        );
      }
    }, "(", ","),
    vscode.languages.registerDefinitionProvider(selector, {
      async provideDefinition(document, position) {
        const target = await atPosition(document, position);
        if (target === undefined) return undefined;
        const values = await vscode.commands.executeCommand<Array<vscode.Location | vscode.LocationLink>>(
          "vscode.executeDefinitionProvider",
          target.entry.uri,
          target.position
        );
        if (values === undefined) return undefined;
        if (values.every((value) => value instanceof vscode.Location)) {
          return values
            .map((value) => mapLocation(target.entry, value as vscode.Location))
            .filter((value): value is vscode.Location => value !== undefined);
        }
        return values
          .filter((value): value is vscode.LocationLink => !(value instanceof vscode.Location))
          .map((value) => mapDefinitionLink(target.entry, value));
      }
    }),
    vscode.languages.registerReferenceProvider(selector, {
      async provideReferences(document, position, context) {
        const target = await atPosition(document, position);
        if (target === undefined) return undefined;
        const values = await vscode.commands.executeCommand<vscode.Location[]>(
          "vscode.executeReferenceProvider",
          target.entry.uri,
          target.position,
          context.includeDeclaration
        );
        return values
          ?.map((value) => mapLocation(target.entry, value))
          .filter((value): value is vscode.Location => value !== undefined);
      }
    }),
    vscode.languages.registerRenameProvider(selector, {
      async provideRenameEdits(document, position, newName) {
        const target = await atPosition(document, position);
        if (target === undefined) return undefined;
        const edit = await vscode.commands.executeCommand<vscode.WorkspaceEdit>(
          "vscode.executeDocumentRenameProvider",
          target.entry.uri,
          target.position,
          newName
        );
        return edit === undefined ? undefined : mapWorkspaceEdit(target.entry, edit);
      }
    }),
    vscode.languages.registerDocumentRangeFormattingEditProvider(selector, {
      async provideDocumentRangeFormattingEdits(document, range, options) {
        const start = await atPosition(document, range.start);
        const end = await atPosition(document, range.end);
        if (start === undefined || end === undefined || start.entry.uri.toString() !== end.entry.uri.toString()) {
          return [];
        }
        const edits = await vscode.commands.executeCommand<vscode.TextEdit[]>(
          "vscode.executeFormatRangeProvider",
          start.entry.uri,
          new vscode.Range(start.position, end.position),
          options
        );
        return edits?.flatMap((edit) => {
          const range = toHostRange(start.entry, edit.range);
          return range === undefined ? [] : [new vscode.TextEdit(range, edit.newText)];
        }) ?? [];
      }
    }),
    vscode.languages.onDidChangeDiagnostics((event) => {
      for (const uri of event.uris) {
        const entry = content.get(uri);
        if (entry === undefined) continue;
        const mapped = vscode.languages.getDiagnostics(uri).flatMap((diagnostic) => {
          const range = toHostRange(entry, diagnostic.range);
          if (range === undefined) return [];
          const copy = new vscode.Diagnostic(
            range,
            diagnostic.message,
            diagnostic.severity
          );
          copy.code = diagnostic.code;
          copy.source = diagnostic.source ?? entry.region.language;
          copy.tags = diagnostic.tags;
          return [copy];
        });
        const hostKey = entry.host.uri.toString();
        const regions = diagnosticsByHost.get(hostKey) ?? new Map<string, vscode.Diagnostic[]>();
        regions.set(uri.toString(), mapped);
        diagnosticsByHost.set(hostKey, regions);
        diagnostics.set(entry.host.uri, [...regions.values()].flat());
      }
    }),
    vscode.workspace.onDidCloseTextDocument((document) => {
      if (document.languageId === "osiris") {
        diagnostics.delete(document.uri);
        diagnosticsByHost.delete(document.uri.toString());
      }
    }),
    vscode.workspace.onDidOpenTextDocument((document) => {
      void refreshHost(document);
    }),
    vscode.workspace.onDidChangeTextDocument((event) => {
      void refreshHost(event.document);
    })
  );

  for (const document of vscode.workspace.textDocuments) {
    void refreshHost(document);
  }
}

function configuredLanguageId(language: string): string {
  const configured = vscode.workspace
    .getConfiguration("osiris")
    .get<Record<string, string>>("embeddedLanguages", {});
  return configured[language] ?? LANGUAGE_IDS[language] ?? language;
}

async function openRegion(entry: RegionDocument, content: EmbeddedContentProvider): Promise<void> {
  content.set(entry);
  let document = await vscode.workspace.openTextDocument(entry.uri);
  const language = configuredLanguageId(entry.region.language);
  if (document.languageId !== language) {
    document = await vscode.languages.setTextDocumentLanguage(document, language);
  }
}

function toEmbeddedPosition(entry: RegionDocument, host: vscode.Position): vscode.Position | undefined {
  const lines = entry.region.text.split("\n");
  const mapping = entry.region.lineMap.find((line) => line.hostLine === host.line);
  if (mapping === undefined || host.character < mapping.hostCharacter) return undefined;
  const text = lines[mapping.embeddedLine] ?? "";
  const character = host.character - mapping.hostCharacter;
  return character <= text.length ? new vscode.Position(mapping.embeddedLine, character) : undefined;
}

function toHostPosition(entry: RegionDocument, embedded: vscode.Position): vscode.Position | undefined {
  const mapping = entry.region.lineMap[embedded.line];
  const line = entry.region.text.split("\n")[embedded.line];
  if (mapping === undefined || line === undefined || embedded.character > line.length) return undefined;
  return new vscode.Position(mapping.hostLine, mapping.hostCharacter + embedded.character);
}

function toHostRange(entry: RegionDocument, range: vscode.Range): vscode.Range | undefined {
  const start = toHostPosition(entry, range.start);
  const end = toHostPosition(entry, range.end);
  return start === undefined || end === undefined ? undefined : new vscode.Range(start, end);
}

function mapLocation(entry: RegionDocument, location: vscode.Location): vscode.Location | undefined {
  if (location.uri.toString() !== entry.uri.toString()) return location;
  const range = toHostRange(entry, location.range);
  return range === undefined ? undefined : new vscode.Location(entry.host.uri, range);
}

function mapDefinitionLink(
  entry: RegionDocument,
  value: vscode.LocationLink
): vscode.LocationLink {
  if (value.targetUri.toString() !== entry.uri.toString()) return value;
  const targetRange = toHostRange(entry, value.targetRange);
  const targetSelectionRange = value.targetSelectionRange === undefined
    ? undefined
    : toHostRange(entry, value.targetSelectionRange);
  if (targetRange === undefined || (value.targetSelectionRange !== undefined && targetSelectionRange === undefined)) {
    return value;
  }
  return {
    originSelectionRange: value.originSelectionRange,
    targetUri: entry.host.uri,
    targetRange,
    targetSelectionRange
  };
}

function mapWorkspaceEdit(entry: RegionDocument, edit: vscode.WorkspaceEdit): vscode.WorkspaceEdit | undefined {
  const mapped = new vscode.WorkspaceEdit();
  for (const [uri, edits] of edit.entries()) {
    if (uri.toString() !== entry.uri.toString()) {
      mapped.set(uri, edits);
      continue;
    }
    const hostEdits: Array<vscode.TextEdit | vscode.SnippetTextEdit> = [];
    for (const item of edits) {
      const range = toHostRange(entry, item.range);
      if (range === undefined) return undefined;
      if (item instanceof vscode.SnippetTextEdit) {
        hostEdits.push(new vscode.SnippetTextEdit(range, item.snippet));
      } else {
        hostEdits.push(new vscode.TextEdit(range, item.newText));
      }
    }
    mapped.set(entry.host.uri, hostEdits);
  }
  return mapped;
}
