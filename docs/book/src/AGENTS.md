<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# src

## Purpose
Markdown source for the operational handbook. `SUMMARY.md` drives the sidebar; every published page must be listed there.

## Key Files
| File | Description |
|------|-------------|
| `SUMMARY.md` | mdBook table of contents — controls order + nesting |
| `introduction.md` | Landing page — what the handbook covers |

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `getting-started/` | Install, configure, first request (see `getting-started/AGENTS.md`) |
| `concepts/` | Core concepts — x402, routing, escrow, pricing (see `concepts/AGENTS.md`) |
| `api/` | HTTP API reference (see `api/AGENTS.md`) |
| `sdks/` | Per-language SDK guides (see `sdks/AGENTS.md`) |
| `operations/` | Ops handbook — deploy, monitor, on-call (see `operations/AGENTS.md`) |

## For AI Agents

### Working In This Directory
- Register every new file in `SUMMARY.md` — unlisted files do not render in the sidebar.
- Use relative links for cross-page references.
- Put source-code examples in fenced code blocks with language identifiers.

### Testing Requirements
```bash
mdbook build
mdbook serve
```

### Common Patterns
- Top-level subdirs = top-level sections in the sidebar.
- Each subdir has its own small `README.md`-style landing page.

## Dependencies

### Internal
_(none direct)_

### External
- `mdbook`.

<!-- MANUAL: -->
