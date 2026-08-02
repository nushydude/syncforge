# Contributing to SyncForge

Thank you for your interest in contributing.

## Development setup

1. Install [Node.js](https://nodejs.org/) (LTS), [pnpm](https://pnpm.io/), and [Rust](https://www.rust-lang.org/tools/install).
2. Clone the repository and install dependencies:

   ```bash
   git clone https://github.com/nushydude/syncforge.git
   cd syncforge
   pnpm install
   ```

3. Run the app in development mode:

   ```bash
   pnpm tauri dev
   ```

## Quality gates

Before opening a pull request for a broad change, run the same checks as CI:

```bash
pnpm run ci:local
```

This runs frontend format/lint/test/build and Rust fmt/clippy/test.

## Releasing a new version

SyncForge uses GitHub Actions to build and publish Windows installers (mirroring [LightFrame](https://github.com/nushydude/lightframe)).

Follow the detailed, deterministic [release guide](docs/RELEASE_GUIDE.md), including the version consistency, release-note, validation, and asset verification checklists below.

1. **Bump version numbers** on a branch — keep all four sources in sync (e.g. `0.1.0` → `0.2.0`):
   - `package.json`
   - `src-tauri/Cargo.toml`
   - `src-tauri/Cargo.lock` (`syncforge` package entry)
   - `src-tauri/tauri.conf.json`
2. **Merge to `main`** via pull request.
3. **Tag and push** to trigger the release workflow:

   ```bash
   git pull origin main
   git tag v0.2.0
   git push origin v0.2.0
   ```

4. **Publish the draft release**: The workflow builds `.msi` and `.exe` installers and attaches them to a **draft** GitHub release (`app-v<version>`). Edit the release notes on GitHub, then publish:
   - In the browser: **Releases** → open the draft → **Publish release**
   - Or via CLI: `gh release edit app-v0.2.0 --draft=false`

### Signing secrets (optional)

For update artifacts and signed installers, configure these repository secrets (same as LightFrame):

- `TAURI_SIGNING_PRIVATE_KEY`
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`

Without them, release builds still run; signing-dependent features are skipped.
