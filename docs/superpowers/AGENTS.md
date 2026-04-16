<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# superpowers

## Purpose
Plans and specs authored via the `superpowers:writing-plans` / `superpowers:write-plan` skills. Kept separate from `docs/plans/` because the format + workflow is different (formal skill-driven structure, reviewer checkpoints baked in).

## Key Files
_(no loose files — see subdirectories)_

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `plans/` | Implementation plans (see `plans/AGENTS.md`) |
| `specs/` | Design specs — typically paired with a plan of the same date (see `specs/AGENTS.md`) |

## For AI Agents

### Working In This Directory
- Use the `superpowers:writing-plans` skill to author new files here — it enforces the expected structure (goal, dependencies, credentials, review checkpoints).
- Every plan should list env vars, manual user actions, infra prerequisites, and reviewer agents in a "What I need from you" section.
- Once approved and executed, keep the plan file as a history record; don't delete.

### Testing Requirements
_(n/a — editorial)_

### Common Patterns
- Filename: `YYYY-MM-DD-slug.md`.
- Related spec + plan share the date slug so they're colocated alphabetically.

## Dependencies

### Internal
- Cross-refs to source modules and migration files.

### External
- `superpowers:writing-plans` skill (invoked via the Skill tool).

<!-- MANUAL: -->
