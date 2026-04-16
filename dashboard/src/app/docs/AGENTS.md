<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# docs

## Purpose
Fumadocs-backed public docs site at `/docs/*`. Content comes from `../../../content/docs/`; the dynamic segment `[[...slug]]` renders any slug under that tree.

## Key Files
| File | Description |
|------|-------------|
| `layout.tsx` | Docs-site layout — navigation, sidebar, TOC, search (wraps Fumadocs' `DocsLayout`) |

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `[[...slug]]/` | Catch-all route — renders a single MDX page from `content/docs/<slug>.mdx` |

## For AI Agents

### Working In This Directory
- Do **not** put content here — content lives in `content/docs/`. This directory is the renderer.
- Sidebar ordering / grouping comes from `content/docs/meta.json` — edit that, not the layout.
- If you need to customize MDX rendering, edit `@/components/mdx.tsx` (shared across docs and dashboard).

### Testing Requirements
```bash
npm --prefix dashboard run dev
# visit http://localhost:3000/docs
```

### Common Patterns
- `DocsLayout` from Fumadocs wraps the content; the catch-all slug is resolved via `source.getPage(slug)`.

## Dependencies

### Internal
- `@/lib/source` (Fumadocs content source), `@/components/mdx`.

### External
- Fumadocs, MDX.

<!-- MANUAL: -->
