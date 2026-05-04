# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Purpose

ontology-tooling is the shared release/version/tag orchestration hub for the `collection-vibe-machine` project. It provides the `ontology-release` command that automates releases for multiple ontology repositories. Releases are local-first (developer workstation or agent), not CI/CD-driven.

## Commands

```bash
mise run check                                          # Smoke check (runs --help)
mise run test                                           # Run test suite (node --test)
mise run release-check -- ../ontology-repo              # Validate repo without mutation
mise run release-dry-run -- ../ontology-repo [bump]     # Preview next release
mise run release -- --repo ../ontology-repo --bump patch  # Execute release
```

npm equivalents: `npm run check`, `npm run test`.

## Architecture

```
bin/ontology-release              → Executable entry point
src/cli/ontology-release.mjs      → CLI implementation (arg parsing, dispatch)
src/lib/release-args.mjs          → Argument parsing & validation
src/lib/versions.mjs              → Semver parsing, bumping, version resolution
src/lib/package-release.mjs       → Release planning & execution (git ops, manifest rewrites)
tests/release-args.test.mjs       → Tests (node:test + assert/strict)
.mise/tasks/                      → Operator entrypoints (check, test, release, etc.)
docs/                             → Architecture docs, playbooks, contracts
```

### Responsibility Boundaries

This repo **owns**: release automation, version planning, manifest path rewrites, release commit/tag creation, push orchestration.

This repo **does not own**: ontology-specific schema semantics, package-local generation, translation logic, or documentation content.

### Release Contract

Target ontology repos must expose three npm scripts:
- `refresh:package-contract` — generate/refresh package manifest
- `validate:bootstrap` — validate manifest and package consistency
- `test:typedb-bootstrap` — validate TypeDB bootstrap

### Release Flow

1. Verify target repo is clean (no uncommitted changes)
2. Compute next version (bump or explicit `--version`)
3. Rewrite versioned manifest paths in `package.json` fields (`manifests`, `provenance.manifest`, `assembly.generatedArtifacts`, `upstream.tag`)
4. Run refresh → validate → test scripts in sequence
5. Create release commit (`Release <pkg> v<version>`) and tag (`v<version>`)
6. Push branch and tag (unless `--no-push`)

### Modes

- `--bump <patch|minor|major>` or `--version <x.y.z>` — full release
- `--dry-run` — plans release without mutating git
- `--validate-only` — runs checks in ephemeral worktree

## Tech Stack & Conventions

- **Node 22** (pinned via mise.toml), ES modules, zero npm dependencies (stdlib only)
- **Testing:** Node's native `node:test` with `assert/strict`; fixtures use temporary git repos
- **Style:** Functional decomposition, async/await, no external linters configured
- **Tags:** `v<semver>` format; commit messages: `Release <name> v<version>`

<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:ca08a54f -->
## Beads Issue Tracker

This project uses **bd (beads)** for issue tracking. Run `bd prime` to see full workflow context and commands.

### Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol
- Use `bd remember` for persistent knowledge — do NOT use MEMORY.md files

## Session Completion

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   bd dolt push
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds
<!-- END BEADS INTEGRATION -->
