# SyncForge

A modern desktop folder-sync tool (SyncToy replacement) built with **Tauri 2**, **React**, **TypeScript**, and **Rust**.

## Prerequisites

- [Node.js](https://nodejs.org/) (LTS)
- [pnpm](https://pnpm.io/)
- [Rust](https://www.rust-lang.org/tools/install) (for `src-tauri`)

## Development

```powershell
pnpm install
pnpm tauri dev
```

Other commands:

| Command | Description |
| --- | --- |
| `pnpm test` | Run Vitest unit tests |
| `pnpm build` | Typecheck and build the Vite frontend |
| `pnpm lint` | ESLint |
| `pnpm format` | Prettier write |
| `cargo build` | Build the Rust crate (from `src-tauri/`) |

Stories and build order live in [`docs/stories/`](docs/stories/README.md).

Automated multi-agent development uses the [Cursor CLI orchestrator](orchestrator/README.md):

```powershell
agent login
.\orchestrator\Run-All.ps1
```

## Project layout

- `src/` — React UI (`main.tsx` wraps `App` in `ErrorBoundary`)
- `src-tauri/` — Tauri/Rust backend (`src/commands/` for invoke handlers)
- `docs/stories/` — User stories S01–S09

## Status

S01 scaffold complete; feature work follows user stories S02–S09.
