<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# book

## Purpose
mdBook-style operational handbook. Builds a browsable HTML site from Markdown source under `src/`, ordered by `src/SUMMARY.md`. Intended for in-team reference; the public product docs live in `../../dashboard/content/docs/`.

## Key Files
| File | Description |
|------|-------------|
| `book.toml` | mdBook config — title, authors, build output dir, preprocessors |

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `src/` | Markdown source, ordered via `SUMMARY.md` (see `src/AGENTS.md`) |

## For AI Agents

### Working In This Directory
- Every new page must be listed in `src/SUMMARY.md` — mdBook does not auto-discover pages.
- Prefer updating an existing section over adding parallel files.
- Build locally with `mdbook build` (or `mdbook serve` for live reload).

### Testing Requirements
```bash
mdbook build    # from docs/book/
mdbook serve    # live preview
```

### Common Patterns
- Relative links between pages; absolute links only for external resources.
- Short, task-focused pages; split when they grow beyond one screen.

## Dependencies

### Internal
- References to source code modules (linked by path).

### External
- `mdbook`.

<!-- MANUAL: -->
