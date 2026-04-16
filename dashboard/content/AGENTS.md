<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# content

## Purpose
Content sources consumed by Fumadocs. Only `docs/` exists today — it powers the public docs site at `/docs`.

## Key Files
_(no loose files)_

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `docs/` | Fumadocs MDX source (see `docs/AGENTS.md`) |

## For AI Agents

### Working In This Directory
- Add new MDX files under `docs/` and register them in the relevant `meta.json` so the sidebar picks them up.
- Use code fences with language identifiers for proper Shiki highlighting.
- Link between docs with relative paths that Fumadocs resolves — don't hand-roll URLs.

### Testing Requirements
```bash
npm --prefix dashboard run dev
# Visit http://localhost:3000/docs and verify the new page renders
```

### Common Patterns
- MDX with frontmatter (title, description, slug-override where needed).
- Short, task-focused pages over long-form essays.

## Dependencies

### Internal
- Consumed by `src/lib/source.ts` and `src/app/docs/[[...slug]]`.

### External
- Fumadocs MDX loader.

<!-- MANUAL: -->
