# Agent-instructions audit

Audited on 2026-08-02.

## Findings addressed

### No canonical repository-level instructions

The repository had orchestrator prompts and contributor guidance, but no root-level agent contract.

**Resolution:** added [`AGENTS.md`](../AGENTS.md) with safety, validation, Git, PR, release, and orchestration rules.

### Release-note behavior was implicit

The release workflow created a generic body while prior releases used structured, user-facing notes. There was no deterministic instruction to inspect previous releases or verify installer links.

**Resolution:** added [`RELEASE_GUIDE.md`](RELEASE_GUIDE.md) with a required structure, evidence rules, version checklist, tag behavior, and publish checklist.

### Version sources were incomplete

Contributor guidance omitted the `syncforge` entry in `Cargo.lock`.

**Resolution:** the release guide requires `package.json`, `Cargo.toml`, `Cargo.lock`, and `tauri.conf.json` to match. CI now checks them automatically.

## Hardening implemented

### Orchestrator branch safety

`Run-Story.ps1` previously used `git checkout -B`, which could rewrite an existing local story branch and did not reject unrelated uncommitted changes.

**Resolution:** the orchestrator now requires a clean worktree, verifies local `main`, switches without rewriting existing branches, and refuses to initialize missing repository history. It remains local-only and does not fetch or push remote state.

### Fail-closed verdict parsing

`Parse-Verdict.ps1` previously searched for verdict text anywhere in agent output and did not enforce the final-line requirement.

**Resolution:** parsing now requires the final non-empty line to be an exact recognized verdict. Unknown implementer, reviewer, or approver output stops the workflow.

### Orchestrator portability

`orchestrator/stories.json` contained an absolute Windows workspace path.

**Resolution:** removed the machine-specific value; the runner defaults to the repository root when no override is configured.

### Agent permission separation

The shared Cursor CLI profile remains broad, but the invocation layer previously gave all roles trusted/MCP-enabled flags.

**Resolution:** implementers retain write/trusted/MCP flags; reviewers and approvers run in ask mode without those flags. The shared profile remains a follow-up because it needs role-specific configuration or isolated worktrees.

### CI version consistency

CI did not compare the four version sources.

**Resolution:** added `scripts/check-version-consistency.mjs` and run it in the frontend quality gate.

### CI and release execution controls

The workflows had no explicit concurrency or release timeout policy.

**Resolution:** CI now cancels superseded runs per ref; releases are serialized and the Windows bundle has a 20-minute timeout.

### Generated-file formatting scope

Generated Tauri schemas could cause repository-wide Prettier checks to drift from source formatting.

**Resolution:** `src-tauri/gen/schemas` is now explicitly excluded in `.prettierignore`.

## Remaining risks and recommended follow-ups

### Remote main freshness

The orchestrator verifies and switches to local `main`, but deliberately does not fetch or fast-forward from a remote because it is documented as local-only.

**Recommendation:** add an explicit opt-in refresh switch if the orchestrator is later used in a shared remote workflow.

### Shared CLI permission profile

`.cursor/cli.json` remains broad because implementers need write access and the file does not support role-specific permissions.

**Recommendation:** introduce separate reviewer/approver profiles or run those roles in isolated read-only worktrees.

### Release-note automation

The release workflow still starts with a generic release body, so final notes must be written before publishing.

**Recommendation:** add release-note validation or generation once the preferred GitHub release-note source is decided.

## Overall assessment

The previous instructions were useful for the local story orchestrator, but incomplete as a repository-wide agent contract. The new root instructions, release guide, preflight checks, CI version validation, and workflow controls close most clarity and safety gaps. Remaining work is primarily remote freshness, role-specific CLI permissions, and optional release-note automation.
