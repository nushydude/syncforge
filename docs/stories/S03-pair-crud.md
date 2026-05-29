# S03 — Folder pair CRUD UI

## Goal

Users can create, edit, and delete folder pairs with sync mode and include/exclude filters.

## Acceptance criteria

- [ ] `tauri-plugin-dialog` folder picker via `pick_folder` command
- [ ] `pairsStore`, components: `PairList`, `PairEditor`, `ModeSelector`, `FilterEditor`
- [ ] CRUD flows call `list_pairs`, `save_pair`, `delete_pair`
- [ ] Validation: left/right paths exist, not identical, name required
- [ ] Vitest tests for store and filter editor logic
- [ ] Empty state when no pairs

## Depends on

S02 persistence and commands
