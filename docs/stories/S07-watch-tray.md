# S07 — Real-time watch and system tray

## Goal

Watch folder pairs for changes and auto-sync; app minimizes to tray.

## Acceptance criteria

- [ ] `watcher.rs`: notify + debounce, triggers preview+run or run for watched pairs
- [ ] System tray icon; minimize-to-tray; quit from tray menu
- [ ] PairEditor toggles: enable watch per pair
- [ ] Debounce prevents sync storms on bulk file drops

## Depends on

S05 run engine
