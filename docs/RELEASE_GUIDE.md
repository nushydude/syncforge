# SyncForge release guide

This guide is the deterministic release procedure for humans and agents.

## 1. Establish the release

1. Start from a clean working tree and update local `main`:

   ```powershell
   git status --short
   git switch main
   git pull --ff-only origin main
   ```

2. Choose the next semantic version. Use a patch release for fixes, a minor release for backwards-compatible features, and a major release for breaking changes.
3. Confirm the previous release tag and inspect the previous two GitHub release bodies. Match their tone and structure unless there is a deliberate reason to change it.

## 2. Update versions

Update all four version locations to exactly the same value:

- `package.json`
- `src-tauri/Cargo.toml`
- `src-tauri/Cargo.lock` (the `syncforge` package entry)
- `src-tauri/tauri.conf.json`

Then run formatting and build checks. Do not tag until the version is consistent.

## 3. Validate

Run the complete local quality gate:

```powershell
pnpm run ci:local
```

Also run the production build if it was not already included by the quality gate:

```powershell
pnpm run build
```

Resolve failures before committing the version bump.

## 4. Commit and tag

Commit the version change on `main`, push it, and create an annotated `v<version>` tag:

```powershell
git add package.json src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/tauri.conf.json
git commit -m "Prepare <version> release"
git push origin main
git tag -a v<version> -m "SyncForge v<version>"
git push origin v<version>
```

The tag `v<version>` triggers the Windows release workflow. The workflow creates the GitHub release with the installer-facing tag `app-v<version>`.

## 5. Write deterministic release notes

Build the notes from the actual range, for example:

```powershell
git log --oneline <previous-tag>..v<version>
```

Use this structure when applicable:

```markdown
# SyncForge v<version>

One-sentence summary of the release value.

## Added

### Feature area

- User-visible additions, grouped by area.

## Improved

- Meaningful usability, performance, or reliability improvements.

## Fixed

- Important bug fixes, especially user-visible or data-safety fixes.

## Quality

- Frontend and Rust checks that actually passed.
- Platform-specific validation or known limitations.

## Downloads

- Direct links to the generated installers.
```

Rules:

- Describe user outcomes, not internal implementation details alone.
- Include only changes supported by the tag range, PR, or verified build output.
- Do not invent test counts, platform support, or signing status.
- Use “Added”, “Improved”, “Fixed”, and “Quality” sections only when they contain real information.
- Mention notable limitations or unsigned artifacts explicitly.
- Link the exact `.exe` and `.msi` assets from the release.
- Preserve the style of the previous releases; do not replace structured notes with a generic placeholder.

## 6. Verify and publish

After the workflow completes:

1. Confirm the run passed.
2. Open `app-v<version>` and verify it is the expected draft release.
3. Confirm both Windows installer assets are present and downloadable.
4. Review the rendered notes, version, tag, and links.
5. Publish the release only after those checks pass:

   ```powershell
   gh release edit app-v<version> --draft=false
   ```

6. Report the release URL, installer links, workflow result, and any signing limitations.
