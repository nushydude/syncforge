# S01 — Project scaffold

## Goal

Bootstrap SyncForge as a Tauri 2 desktop app with React 18, TypeScript, Vite, pnpm, Vitest, ESLint, and Prettier. The app should compile and show a minimal shell window.

## Acceptance criteria

- [ ] `pnpm create tauri-app` (or equivalent) with React + TypeScript + Vite template
- [ ] `pnpm install` succeeds; `pnpm tauri dev` launches a window with placeholder UI
- [ ] `pnpm test` runs Vitest (at least one smoke test)
- [ ] `pnpm build` and `cargo build` in `src-tauri` succeed
- [ ] `.gitignore`, `README.md`, ESLint, Prettier, `tsconfig.json` present
- [ ] `src/main.tsx` renders `App` inside `ErrorBoundary`
- [ ] Local commit on story branch when orchestrator approves

## Out of scope

- Sync logic, database, folder pairs

## Notes

Reference stack: lightframe (Tauri 2 + React + TS + Vite). D: drive reference may be offline; use standard Tauri 2 template if configs cannot be copied.
