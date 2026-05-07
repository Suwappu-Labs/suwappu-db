---
description: Tag and ship a release — verify clean state, run checks, tag, push, create GitHub release
argument-hint: <version e.g. v0.2.0>
allowed-tools: Bash, Read, Edit
---

# Release $ARGUMENTS

You are preparing to release **$ARGUMENTS**.

## Phase 1: Pre-flight (NO EXCEPTIONS)

Run every check. Report findings. Do not proceed if any fail.

```bash
git rev-parse --abbrev-ref HEAD              # must be main or release branch
git status --porcelain                        # must be clean
git rev-parse HEAD                            # note for release notes
git rev-parse @{u}                            # must equal HEAD (no divergence)
gh pr list --state open --base main           # surface open PRs
gh run list --branch main --limit 5           # recent CI must be green
```

If any of these fails or returns unexpected output, **stop** and report.

## Phase 2: Verify

Run `/check`. All steps must be green.

## Phase 3: Version bump

1. Find version strings (`Cargo.toml` workspace + member crates, `README.md` install instructions).
2. Bump to `$ARGUMENTS` (without leading `v` in Cargo.toml — i.e. `version = "0.2.0"`).
3. Update `CHANGELOG.md` — move "Unreleased" entries under the new version with today's date.
4. Run `cargo update --workspace` to refresh `Cargo.lock`.
5. Run `cargo build --release` — must compile.
6. Commit: `chore: release $ARGUMENTS`.

## Phase 4: Tag and push

These are in the `ask` permission tier — user will confirm:

```bash
git tag -a $ARGUMENTS -m "Release $ARGUMENTS"
git push origin main
git push origin $ARGUMENTS
```

## Phase 5: GitHub release

```bash
gh release create $ARGUMENTS \
  --title "$ARGUMENTS" \
  --notes-file <(awk '/^## /{c++} c==2{exit} c>=1' CHANGELOG.md)
```

The notes-file extracts the topmost CHANGELOG section. Verify the extracted content before running.

## Phase 6: Confirm

Run `gh release view $ARGUMENTS` and report the release URL.
