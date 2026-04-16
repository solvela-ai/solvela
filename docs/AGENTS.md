<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# docs

## Purpose
Repository-internal documentation: operational handbooks, implementation plans, research notes, product specs, and load-test results. This is distinct from the **public** docs site, which lives under `../dashboard/content/docs/` and is what end-users read on solvela.vercel.app.

## Key Files
_(no loose files — see subdirectories)_

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `book/` | mdBook-style handbook (getting-started, concepts, API, SDKs, operations) (see `book/AGENTS.md`) |
| `plans/` | Implementation plans — one markdown file per initiative (see `plans/AGENTS.md`) |
| `product/` | Product strategy, use cases, FAQ, regulatory position (see `product/AGENTS.md`) |
| `research/` | Research notes and external investigations (see `research/AGENTS.md`) |
| `load-tests/` | Load-test plans + results (see `load-tests/AGENTS.md`) |
| `superpowers/` | Plans + specs authored via the `superpowers` skill (see `superpowers/AGENTS.md`) |

## For AI Agents

### Working In This Directory
- Public docs live in `../dashboard/content/docs/`. Do **not** duplicate material here — link instead.
- Plans: prefer updating an existing plan's status section over spawning a new file for each iteration; when a plan lands, note outcome in HANDOFF.md / CHANGELOG.md and leave the plan as-is for history.
- Filenames use the `YYYY-MM-DD-slug.md` convention in `plans/`, `load-tests/`, `research/`, and `superpowers/plans/`.

### Testing Requirements
No automated tests — reviews are the quality gate. Some docs may render via mdBook (`docs/book/`).

### Common Patterns
- Dated filenames for plans/research; undated for evergreen reference material (product FAQ, how-it-works).
- GitHub-flavoured Markdown throughout.

## Dependencies

### Internal
- Cross-references to source modules and public docs.

### External
_(none for Markdown; mdBook for `book/`)_

<!-- MANUAL: -->
