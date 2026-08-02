# SyncForge CLI Orchestrator

Runs each user story through **three independent Cursor CLI agents** (all `composer-2.5`):

1. **Implementer** — builds the story (`agent -p`, full agent mode, `--force --trust`)
2. **Reviewer** — fresh session, read-only (`--mode ask`), iterates until `REVIEW_VERDICT: APPROVED`
3. **Approver** — fresh session, read-only, final gate (`APPROVE_VERDICT: YES`)

No shared chat context between roles. Each `agent -p` invocation is a new session.

## Prerequisites

```powershell
# Install CLI (once)
irm 'https://cursor.com/install?win32=true' | iex

# Authenticate (once, opens browser)
agent login

# Verify
agent status
agent models
```

## Run

```powershell
cd C:\Users\CDAND\Projects\syncforge

# All stories (non-stop until done or failure)
.\orchestrator\Run-All.ps1

# Single story
.\orchestrator\Run-Story.ps1 -StoryId S01

# Resume from a story
.\orchestrator\Run-All.ps1 -StartFrom S04
```

## Stop gracefully

Create `orchestrator/STOP` — the current story exits cleanly; remove the file to resume.

## State and logs

| Path                         | Purpose                             |
| ---------------------------- | ----------------------------------- |
| `orchestrator/state.json`    | Per-story status, iteration count   |
| `orchestrator/logs/<story>/` | Prompts + agent output per role     |
| `docs/stories/`              | Story specs and acceptance criteria |
| `orchestrator/stories.json`  | Machine-readable story order        |

## Git workflow

- Each story uses branch `story/Sxx-...`
- On approve: merge `--no-ff` into `main` locally
- No remote push (by design)

## Model

Default: `composer-2.5` in `orchestrator/stories.json`. Change there if your account uses a different ID (`agent models`).
