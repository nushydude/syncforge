# SyncForge

A modern desktop folder-sync tool (SyncToy replacement) built with **Tauri 2**, **React**, **TypeScript**, and **Rust**.

## Features

- **Folder pairs** — Left/right folders with Synchronize, Echo, and Contribute modes
- **Preview before run** — See planned copies, updates, and deletes before executing
- **Sync engine** — Safe copies, Recycle Bin deletes, optional hash verification
- **Conflict policies** — Newer-wins, left/right, keep-both, or prompt
- **Real-time watch** — Auto-sync on file changes with debouncing
- **Scheduling** — Cron-based runs with desktop notifications
- **History** — Past runs with detail view and CSV/JSON export

## Installation

Download the latest Windows installer from the [Releases](https://github.com/nushydude/syncforge/releases) page.

> **Windows:** New or unsigned apps may show SmartScreen (“Windows protected your PC”). Click **More info**, then **Run anyway**.

## Development

### Prerequisites

- [Node.js](https://nodejs.org/) (LTS)
- [pnpm](https://pnpm.io/) (`corepack enable` recommended)
- [Rust](https://www.rust-lang.org/tools/install)

### Run locally

```bash
git clone https://github.com/nushydude/syncforge.git
cd syncforge
pnpm install
pnpm tauri dev
```

| Command | Description |
| --- | --- |
| `pnpm test` | Vitest unit tests |
| `pnpm build` | Typecheck and build the Vite frontend |
| `pnpm lint` | ESLint |
| `pnpm format` | Prettier write |
| `pnpm ci:local` | Full local CI gate (frontend + Rust) |
| `cargo build` | Build the Rust crate (from `src-tauri/`) |

### Production build

```bash
pnpm tauri build
```

Installers are written under `src-tauri/target/release/bundle/`.

## Releasing

See [CONTRIBUTING.md](CONTRIBUTING.md#releasing-a-new-version) for version bumps, `v*` tags, and draft GitHub releases.

## Project layout

- `src/` — React UI
- `src-tauri/` — Tauri/Rust backend
- `docs/stories/` — User stories (S01–S09)
- `orchestrator/` — Optional Cursor CLI multi-agent build loop

## License

See repository license file when published.
