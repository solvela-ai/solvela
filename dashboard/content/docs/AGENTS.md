<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# docs

## Purpose
MDX content for the public docs site at `/docs`. Fumadocs traverses this tree, reads `meta.json` for ordering + grouping, and renders pages through `src/app/docs/[[...slug]]`.

## Key Files
| File | Description |
|------|-------------|
| `index.mdx` | Docs home page |
| `quickstart.mdx` | 5-minute quickstart (install SDK → make first paid call) |
| `meta.json` | Top-level ordering + section groupings for the sidebar |

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `api/` | HTTP API reference — endpoints, 402 shape, headers |
| `concepts/` | Conceptual docs — x402, Solvela vs traditional APIs, pricing, escrow |
| `sdks/` | Per-SDK quickstarts (Python, TypeScript, Go, MCP) |

## For AI Agents

### Working In This Directory
- Every new MDX file needs to appear in the relevant `meta.json` — Fumadocs does not auto-list files.
- Keep pages focused on one task; split long docs into a folder with its own `meta.json`.
- Use Fumadocs callouts / tabs via the components exposed in `src/app/components/docs/` or `src/components/mdx.tsx`.
- Code blocks: include a language tag so Shiki highlights correctly.

### Testing Requirements
```bash
npm --prefix dashboard run dev
# Visit http://localhost:3000/docs/<slug>
```

### Common Patterns
- Frontmatter: `title`, `description`, optional `icon`.
- Relative links between pages (Fumadocs resolves them).

## Dependencies

### Internal
- Fumadocs renderer at `src/app/docs/[[...slug]]`.

### External
- Fumadocs MDX, Shiki.

<!-- MANUAL: -->
