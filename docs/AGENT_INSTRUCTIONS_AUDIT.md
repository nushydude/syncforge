# Agent-instructions audit

Audited on 2026-08-02.

## Findings addressed

### No canonical repository-level instructions

The repository had orchestrator prompts and contributor guidance, but no root-level agent contract. This made it unclear which rules applied to agents outside the story orchestrator.

**Resolution:** added [`AGENTS.md`](../AGENTS.md) with safety, validation, Git, PR, release, and orchestration rules.

### Release-note behavior was implicit

The release workflow created a generic body (`See the assets to download the installer.`), while prior releases used structured, user-facing notes. There was no deterministic instruction to inspect previous releases or verify installer links.

**Resolution:** added [`RELEASE_GUIDE.md`](RELEASE_GUIDE.md) with a required structure, evidence rules, version checklist, tag behavior, and publish checklist.

### Version sources were incomplete

Contributor guidance listed `package.json`, `Cargo.toml`, and `tauri.conf.json`, but omitted the root `syncforge` entry in `Cargo.lock`.

**Resolution:** the release guide requires all four locations to match.

## Remaining risks and recommended follow-ups

### Orchestrator branch safety

`Run-Story.ps1` uses `git checkout -B`, which can reset an existing local story branch to the current commit. It also does not refuse to start with unrelated uncommitted changes or refresh `main` from the remote.

**Recommendation:** add a preflight that requires a clean worktree, verifies the current repository, fast-forwards `main`, and refuses to overwrite an existing branch unless explicitly requested.

### Orchestrator verdict parsing

`Parse-Verdict.ps1` searches for verdict text anywhere in agent output. It does not enforce the prompts’ “exactly one final line” requirement, and an implementer output with no recognized verdict is not treated as blocked.

**Recommendation:** parse only an anchored final verdict line and fail closed on `UNKNOWN` for every role.

### Orchestrator configuration portability

`orchestrator/stories.json` contains an absolute Windows workspace path. A different checkout or operating system will not use the configured repository automatically.

**Recommendation:** default the workspace to the repository root and allow an explicit override through a parameter or environment variable.

### Agent permissions are broad

`.cursor/cli.json` allows all Shell, Read, Write, and MCP operations, while `Invoke-Agent.ps1` runs trusted agents with MCP approval enabled. The prompts add behavioral constraints, but the permissions are not technically restricted.

**Recommendation:** use a least-privilege profile for reviewers and approvers, and require explicit confirmation for destructive filesystem or remote Git operations.

### CI does not enforce version consistency

CI validates formatting, lint, tests, and builds, but it does not compare the four version sources or validate release-note presence for a version tag.

**Recommendation:** add a small CI check that compares package, Cargo, lockfile, and Tauri versions, and optionally checks that a tagged release has non-placeholder notes before publication.

### Release workflow has no explicit concurrency or timeout policy

The workflow relies on the default GitHub Actions behavior. A stuck Windows build can consume a runner indefinitely, and two tags could publish concurrently.

**Recommendation:** add workflow-level `concurrency` and a job timeout appropriate for the Windows bundle.

### Local and CI formatting scope can drift

The repository-wide `pnpm run format:check` currently flags four tracked generated Tauri schema files in `src-tauri/gen/schemas/`, even though the documentation files in this change pass targeted formatting. Generated-file ownership and whether generated schemas must be formatted are not stated.

**Recommendation:** either include generated schemas in the formatting contract and format them in CI, or exclude generated files consistently in `.prettierignore` and document when they are regenerated.

## Overall assessment

The previous instructions were useful for the local story orchestrator, but incomplete as a repository-wide agent contract. The new root instructions and release guide close the immediate clarity gap. The remaining items are implementation hardening opportunities, especially branch safety, fail-closed verdict parsing, and least-privilege agent execution.
