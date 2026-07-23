# Release guide

Osiris has one source version and two independent release channels. Update
`Cargo.toml` and `editors/vscode/package.json` deliberately; each workflow
rejects a tag that does not match its own package version.

## PyPI Trusted Publisher

The Python distribution is `osiris-lang`. It installs the `osiris` Python
package and the `osr` command.

Configure the pending Trusted Publisher on PyPI with these exact values:

| Field | Value |
| --- | --- |
| PyPI Project Name | `osiris-lang` |
| Owner | `mjason` |
| Repository name | `osiris` |
| Workflow name | `publish-pypi.yml` |
| Environment name | `pypi` |

Create a GitHub Actions environment named `pypi` under **Settings >
Environments**. No PyPI API token is required: the publish job uses GitHub OIDC
with `id-token: write`.

After the version is committed and pushed, publish with:

```console
git tag v0.1.0
git push origin v0.1.0
```

The tag triggers `.github/workflows/publish-pypi.yml`, which checks the version,
runs the Rust tests, builds native CLI wheels and an sdist, then publishes through the
`pypi` environment.

## VS Code VSIX

The VS Code extension is not published to Marketplace yet. A dedicated tag
builds the VSIX and creates a GitHub Release containing it:

```console
git tag vscode-v0.1.0
git push origin vscode-v0.1.0
```

The tag must equal `vscode-v` plus the version in
`editors/vscode/package.json`. The workflow attaches
`osiris-vscode-0.1.0.vsix` to the generated GitHub Release. It needs only the
repository-provided `GITHUB_TOKEN`; no Marketplace token is involved.
