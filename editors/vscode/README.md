# Osiris for VS Code

This extension adds `.osr` and `.osri` language registration, syntax
highlighting, bracket configuration, and an LSP client for the Osiris
Lisp-to-Python compiler. Diagnostics, completion, hover, signature help,
definition, references, and rename are provided by `osr lsp`.

## Install

1. Install the compiler and language server:

   ```console
   uv tool install osiris-lang
   ```

2. Open the repository's [GitHub Releases](https://github.com/mjason/osiris/releases)
   page and select the latest tag named `vscode-vX.Y.Z`.
3. Download the attached `osiris-vscode-X.Y.Z.vsix` file.
4. In VS Code, run **Extensions: Install from VSIX...** and select the file.

The extension starts `osr lsp` from `PATH`. If it is installed elsewhere,
set `osiris.server.path` to the executable's absolute path and run
**Osiris: Restart Language Server**. This extension requires `osr` 0.3.0 or
newer and stops an older server before it can publish incompatible diagnostics.

## Settings

- `osiris.server.path`: executable used to start the language server.
- `osiris.server.arguments`: defaults to `["lsp"]`.
- `osiris.server.siteRoots`: optional package roots used for locked extensions.
- `osiris.displayLocale`: Rich Metadata display locale; empty follows VS Code.
- `osiris.trace.server`: LSP protocol tracing for diagnostics.

## Development

```console
cd editors/vscode
npm ci
npm run check
npm run package
```
