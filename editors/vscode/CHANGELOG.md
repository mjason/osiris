# Changelog

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
