// Jump from generated Python back to the Osiris source that produced it.
//
// Every generated `.py` ships with a sibling `.py.map` naming the authored
// `.osr` and mapping generated line/column ranges to byte spans in it. The
// provider activates only when that sibling exists, so ordinary Python
// projects never see it.

import * as fs from "fs/promises";
import * as vscode from "vscode";

interface GeneratedPosition {
  line: number; // 1-based
  column: number; // 0-based
}

interface SourceMapping {
  generated_start: GeneratedPosition;
  generated_end: GeneratedPosition;
  source_span: { start: number; end: number };
}

interface OsirisSourceMap {
  version: number;
  source: string;
  mappings: SourceMapping[];
}

function covers(mapping: SourceMapping, line: number, column: number): boolean {
  const { generated_start: start, generated_end: end } = mapping;
  if (line < start.line || line > end.line) {
    return false;
  }
  if (line === start.line && column < start.column) {
    return false;
  }
  if (line === end.line && column > end.column) {
    return false;
  }
  return true;
}

function span(mapping: SourceMapping): number {
  const lines = mapping.generated_end.line - mapping.generated_start.line;
  const columns = mapping.generated_end.column - mapping.generated_start.column;
  return lines * 10_000 + columns;
}

/** Byte offset in an .osr source to a UTF-16 editor position. */
function byteOffsetToPosition(source: Buffer, offset: number): vscode.Position {
  const prefix = source.subarray(0, Math.min(offset, source.length));
  const text = prefix.toString("utf8");
  let line = 0;
  let lastLineStart = 0;
  for (let index = 0; index < text.length; index += 1) {
    if (text[index] === "\n") {
      line += 1;
      lastLineStart = index + 1;
    }
  }
  return new vscode.Position(line, text.length - lastLineStart);
}

export function registerGeneratedSourceNavigation(
  context: vscode.ExtensionContext
): void {
  context.subscriptions.push(
    vscode.languages.registerDefinitionProvider(
      { language: "python", scheme: "file" },
      {
        async provideDefinition(document, position) {
          const mapPath = `${document.uri.fsPath}.map`;
          let parsed: OsirisSourceMap;
          try {
            parsed = JSON.parse(await fs.readFile(mapPath, "utf8"));
          } catch {
            return undefined; // Not Osiris output; stay out of the way.
          }
          if (parsed.version !== 3 || typeof parsed.source !== "string") {
            return undefined;
          }
          const line = position.line + 1;
          const candidates = parsed.mappings.filter((mapping) =>
            covers(mapping, line, position.character)
          );
          if (candidates.length === 0) {
            return undefined;
          }
          const narrowest = candidates.reduce((best, next) =>
            span(next) < span(best) ? next : best
          );
          let sourceBytes: Buffer;
          try {
            sourceBytes = Buffer.from(await fs.readFile(parsed.source));
          } catch {
            return undefined;
          }
          const start = byteOffsetToPosition(
            sourceBytes,
            narrowest.source_span.start
          );
          const end = byteOffsetToPosition(
            sourceBytes,
            narrowest.source_span.end
          );
          return new vscode.Location(
            vscode.Uri.file(parsed.source),
            new vscode.Range(start, end)
          );
        },
      }
    )
  );
}
