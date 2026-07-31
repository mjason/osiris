# Changelog

## 0.3.27

- Project builds (`osr build`, `osr run`) now emit all compiler-linked runtime support into one shared `<outDir>/__osiris_runtime__` tree instead of one per top-level package; wheels keep the per-owning-package layout they need to be co-installable. `osr compile --runtime-layout shared|package` selects explicitly.
- Excluding a whole top-level package via `wheel-exclude` now removes its runtime tree too, instead of failing on a manifest that references removed source maps.

## 0.3.26

- Link an external extension's runtime into the consumer's own output instead of importing the provider's installed private `__osiris_runtime__` package: generated imports now name `<owning>.__osiris_runtime__.packages.<provider-id>.…` and the provider file is copied there, so the output no longer requires the provider's package layout at runtime. First-party runtime imports within one build are unchanged.

## 0.3.25

- The language server now resolves extensions from the project's own installed environment (`.venv`) in addition to any editor-configured site roots, matching the CLI. Files `osr check` accepted no longer show missing-module diagnostics for installed extensions in the editor.

## 0.3.24

- A bare `osr` resolved through PATH now hands the invocation to the project's own compiler — the activated `VIRTUAL_ENV`, then a `.venv` on the ancestor path — so what answers is always what the project locked. Explicit paths run exactly the named binary; `OSR_NO_DELEGATE=1` opts out.

## 0.3.23

- Add `osr clean`: a dedicated `outDir` is removed whole; under `outDir: "."` only manifest-tracked artifacts are deleted, never authored files. The `.osiris/` cache goes in both modes.
- Record the in-place publication manifest as versioned JSON (`.osiris/published-artifacts.json`); the 0.3.22 plain-text form is read once and migrated.

## 0.3.22

- Support `outDir: "."`: artifacts land beside the sources at the project root, published in place — only artifacts a previous build recorded are ever cleaned up, never an authored file.
- Ship every optional `.gitignore` rule commented (`outDir` whole-directory, `*.osri`, `*.py.map`), to be opened as needed; the `outDir: "."` layout gets only the per-suffix rules.

## 0.3.21

- Treat the same locked extension dependency installed at two site roots — the project venv plus an isolated build environment — as one provider instead of a conflict, which made every editable install of a project with an extension dependency fail.
- Generate a `.gitignore` on `osr init`; an existing one only gains the machine-local `.osiris/` cache entry.

## 0.3.20

- Add `[tool.osiris] wheel-exclude` to `pyproject.toml`: fnmatch patterns over module paths whose artifacts stay out of the wheel. The modules still compile and still ship in the sdist — the way a project keeps its test modules out of what it distributes.

## 0.3.19

- Stop discovering the project's own installed copy as one of its extensions. uv installs the root project into its environment, so after the first `uv sync` every source module was reported as a duplicate of its own stale interface.

## 0.3.18

- Give the build backend the Osiris-to-Python name translation and use it wherever a declared module name meets an archive path, so a module like `my-pkg.core` can be packaged. The backend previously demanded the two spellings be literally equal, which rejected every module name the mapping changes.

## 0.3.17

- Add `py/embed`, which names an embedded Python provider by file so the Python can be written, read and tested as an ordinary `.py`.
- Stop shipping a `py/embed` source twice in a wheel, once as itself and once inside the generated module.
- Decide a module's public surface through one entry point, so a per-item `^:export` marker cannot be honoured in some places and ignored in others.

## 0.3.16

- Add `osr lsc name`, which reports what an Osiris name becomes in generated Python.
- Validate a distribution name in the build backend before normalizing it, matching the compiler.

## 0.3.15

- Name the source distribution with PEP 625 escaping, which an index requires.

## 0.3.14

- Fix a crash in the build backend when a lock file uses a `~=` compatible-release specifier.

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
