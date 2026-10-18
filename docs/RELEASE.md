# Releasing Kawruh

## What runs where

| Workflow | Trigger | Does |
|---|---|---|
| `.github/workflows/ci.yml` | push to `main`, pull requests | Ubuntu: fmt, clippy, all Rust tests, converter contract, JS tests, schema drift check. Windows: clippy and tests for the desktop app. |
| `.github/workflows/release.yml` | push of a `v*` tag | Drafts a GitHub release, builds the apps and server binaries, uploads `SHA256SUMS.txt`, publishes. |

Release assets:

- `Kawruh_<version>_universal.dmg`: macOS 11+, Apple Silicon and Intel
- `Kawruh_<version>_x64-setup.exe`: Windows installer (NSIS)
- `kawruh-server-v<version>-aarch64-unknown-linux-musl.tar.gz` and `-x86_64-…`: static server binaries
- `SHA256SUMS.txt`

A tag with a suffix (`v0.2.0-beta.1`) is published as a pre-release.

## Cut a release

```sh
scripts/release.sh 0.2.0                 # bumps Cargo.toml, app/package.json, tauri.conf.json, Cargo.lock; commits; tags v0.2.0
git push --atomic origin main v0.2.0     # starts the release workflow
```

The workflow refuses a tag that doesn't match the manifests (`scripts/release.sh --check`).
If a job fails, fix it and re-run the failed jobs: the draft release is reused and
uploads overwrite.

## Deploy the server from a release

The homelab isn't reachable from GitHub's runners, so deploys start from your machine:

```sh
KAWRUH_SSH_PASS=… deploy/deploy.sh --release v0.2.0     # downloads, verifies the checksum, installs, restarts
KAWRUH_SSH_PASS=… deploy/deploy.sh --rollback           # back to the previous binary
```

`deploy/deploy.sh` without `--release` still builds from your checkout.

## Repository settings

All optional. Without any of them the release still builds.

| Name | Kind | Purpose |
|---|---|---|
| `KAWRUH_DEFAULT_SERVER` | variable | Prefills the server field on the sign-in screen, e.g. `http://<host>:7420`. It is compiled into the downloadable app, so leave it unset if the address should stay private. |
| `APPLE_CERTIFICATE` | secret | Developer ID Application certificate, `.p12` as base64 |
| `APPLE_CERTIFICATE_PASSWORD` | secret | Password of that `.p12` |
| `APPLE_SIGNING_IDENTITY` | secret | e.g. `Developer ID Application: Name (TEAMID)` |
| `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | secrets | Notarization (`APPLE_PASSWORD` is an app-specific password) |

## Unsigned builds

Without the Apple secrets the macOS app is ad-hoc signed. macOS blocks it the
first time: open it, then go to System Settings → Privacy & Security → Open Anyway.
Or clear the quarantine flag:

```sh
xattr -dr com.apple.quarantine /Applications/Kawruh.app
```

The Windows installer is unsigned too, so SmartScreen asks once (More info → Run anyway).
