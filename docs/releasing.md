# Release versions from Git tags

Push the intended release tag on the source commit. The release workflow synchronizes version files only in its temporary build checkouts. It does not commit, push or update version files on any repository branch.

Desktop release, including the bundled CLI artifacts:

```bash
git tag -a v0.8.0 -m "ByteTide 0.8.0"
git push origin v0.8.0
```

Standalone CLI release:

```bash
git tag -a cli-v0.8.1 -m "ByteTide CLI 0.8.1"
git push origin cli-v0.8.1
```

Commit and push the desired source changes before tagging. You do not need to run `npm run bump` or commit version-only changes. Canonical SemVer tags are accepted, including prerelease/build suffixes; release visibility and prerelease flags still follow the existing workflow/manual release settings. Do not move an already-published tag.

## What CI changes

Every release build starts from a separate checkout, so version preparation runs independently in verify, Windows, macOS and all CLI targets. Reusable CLI builds inherit the caller's desktop `v*` tag. A standalone CLI build uses its own `cli-v*` tag.

| Location | Version source |
|---|---|
| `package.json` | SemVer extracted from the tag |
| `Cargo.toml` workspace package | Same version; core, CLI and desktop inherit it |
| `src-tauri/tauri.conf.json` | Same version, including DMG naming |
| `package-lock.json` top-level and root package | Same version; dependency entries are preserved |
| `Cargo.lock` workspace package entries | Refreshed by `cargo update --workspace` |

Binary `--version`, GUI version, DMG naming and signed update metadata therefore use the release version, even when the source branch retains a different development version. CLI assets published alongside a desktop release use that desktop tag's version.

Invalid tags and Cargo lock refresh errors fail the job. The existing version-consistency check still runs after preparation; standalone CLI tags are now checked too. Ordinary branch CI continues checking consistency of the committed development manifests and never rewrites them. `npm run bump` remains available for optional local development version changes.

## Tests

```bash
npm run test:release-version
```

The tests cover both tag namespaces, canonical SemVer, npm lock metadata, idempotence and error propagation. With Cargo installed, a temporary dependency-free workspace verifies that all inherited Rust package versions and Cargo.lock are refreshed correctly. Without Cargo that integration test is explicitly skipped. A separate read-only CI job also prepares an archived copy of the real repository and checks that the original checkout has no diff.
