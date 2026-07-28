# Changelog

## 0.3.13

- Write `PKG-INFO` into the source distribution, which PEP 517 requires and an index rejects an upload without.

## 0.3.12

- Align diagnostic carets and formatter line budgets to display columns, so CJK and mixed-width source is laid out and pointed at correctly.
- Keep macro documentation and example spans out of the semantic interface hash, so editing a macro's docs no longer forces dependents to recompile.
- Publish a declaration with the per-item `^:export` marker, including macros, embedded data blocks, `extern` members, and static-record owners.

## 0.3.11

- Avoid initializing project language services for generic requests while retaining project-symbol preflight for project API questions.

## 0.3.10

- Reuse unchanged workspace analysis so hover and definition requests are not blocked by repeated full-project compilation.
- Improve navigation across Unicode paths and strengthen LSA project-symbol preflight for project API questions.

## 0.3.9

- Use the official DeepSeek endpoint and native tool calling for project-aware LSA requests.
- Improve LSA JSON reliability, compiler-validated examples, and bounded LSC evidence.
- Add configurable thinking, reasoning effort, and streamed provider responses to `osiris.jsonc`.

## 0.3.7

- Add project-aware symbol, definition, signature, reference, and source context for LSA through the LSC/LSP boundary.
- Validate `osiris.jsonc` with the published project schema and surface project-loading diagnostics consistently.
- Add the cached libSQL semantic graph with automatic refresh and explicit cache recovery commands.

## 0.3.6

- Upgrade the VS Code language client and its transitive dependencies to resolve the reported high-severity security advisories.
- Require VS Code 1.91 or newer to match the supported host range of the updated language client.

## 0.3.5

- Add the Language Server Agent for compiler-validated explanations, examples, follow-up sessions, and captured runtime results.
- Validate generated examples in the complete project workspace, including imports, embedded Python, standard runtime support, and records resolution.
- Ship and continuously test a complete multi-module `uv` demo project.

## 0.3.4

- Present localized names, migration aliases, signatures, and examples more clearly through LSP and LSC.
- Keep formatting aligned for wide Unicode identifiers and the canonical Osiris style.
- Require the matching compiler release with parallel builds, validated project caching, and more readable generated Python.

## 0.3.3

- Load public standard-library source from the validated installed resource tree.
- Keep Kernel source compiler-owned while preserving navigable standard source URIs.
- Align examples and syntax documentation with OEP-0004 documentation metadata.

## 0.3.2

- Improve standard-library hover and LSC output with task-oriented examples.
- Resolve implicit core symbols correctly during hover and navigation.
- Open exact split standard-library source files from definitions.

## 0.2.0

- Require the compatible Osiris 0.3 language server before starting the client.
- Keep formatting, diagnostics, completion, hover, signatures, navigation, and
  rename behavior aligned with the native `osr lsp` implementation.
- Improve project configuration and localized Rich Metadata integration.

## 0.1.0

- Register `.osr` and `.osri` files.
- Add Osiris syntax highlighting and editing configuration.
- Start `osr lsp` and expose restart, locale, and extension-root settings.
