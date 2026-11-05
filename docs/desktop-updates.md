# Desktop updates

ByteTide supports explicit in-app updates for Windows x86_64 portable EXE and macOS Apple Silicon. The version menu offers Download update, byte/percentage progress and Restart and install. Downloading does not stop connections. Restart requires confirmation, then stops active operations and flushes recordings before replacing the application. Other platforms and unsigned/legacy releases retain the GitHub download-page fallback.

Windows still distributes one `ByteTide.exe`. During installation a temporary copy acts as the helper before normal GUI/single-instance initialization. It waits for the old process, verifies the signed staged update, keeps a backup, replaces the EXE and restarts it. Startup acknowledgement is required before cleanup; failed replacement/startup restores the previous EXE. Do not terminate the helper while replacing files. The EXE directory must be writable; no silent elevation is performed.

macOS continues to distribute a DMG for first installation. In-app updating uses Tauri's official signed `.app.tar.gz` installer and restarts the app. Run the installed app from a writable location rather than directly from the mounted DMG. Updater signatures authenticate the publisher separately from Apple's code signing/Gatekeeper; existing ad-hoc signing remains unchanged.

## Publisher setup

Generate a signing key locally with the repository's pinned Tauri CLI:

```bash
npm ci
npx tauri signer generate -w /secure/path/bytetide-update.key
```

Keep the private key and password outside the repository. Back them up securely; installed builds trust their embedded public key. Configure repository Settings → Secrets and variables → Actions:

| Setting | Kind | Value |
|---|---|---|
| `BYTETIDE_UPDATER_PUBLIC_KEY` | Actions variable | Full contents of generated `.pub` file |
| `TAURI_SIGNING_PRIVATE_KEY` | Actions secret | Full contents of generated private key file |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Actions secret | Password for the private key; empty for an unencrypted key |

Release builds embed the public key through `BYTETIDE_UPDATER_PUBLIC_KEY`. Neither key configured means ordinary manual-download releases. Configuring only one key fails the release preflight rather than publishing incomplete updates. Never place private keys in `tauri.conf.json`, a frontend environment variable or a Git commit.

The `v*` release workflow signs:

- `ByteTide.exe` → `ByteTide.exe.sig`.
- The sealed macOS app archive `ByteTide.app.tar.gz` → `.sig`.
- `desktop-update.json` → `desktop-update.json.sig`.

The manifest references exact tag-specific HTTPS assets, not a mutable latest URL, and contains inline artifact signatures for `windows-x86_64` and `darwin-aarch64`. The application authenticates the manifest and checks its version/platform before trusting the advertised artifact. Signing the manifest binds versions to artifacts even with older Tauri CLI signatures that do not embed a trusted version comment. CLI-only `cli-v*` releases are ignored by desktop update checking.

Publish the complete draft Release only after reviewing all artifacts. No separate update server is required. Existing versions without the updater/public key must be manually upgraded once to a signed-key-enabled build; a PR branch is not an update source.

## Verification before publishing

CI covers frontend state transitions, cancellation, signature/version validation and replacement/rollback helpers, plus the existing workspace checks. Before the first public signed update, test two signed versions on Windows and macOS: download while monitoring, cancel restart, confirm restart, verify recording closure, and verify replacement/relaunch. Also test a locked/read-only destination, an altered signature and a failed startup. The private signing key must not be exposed to pull-request builds.
