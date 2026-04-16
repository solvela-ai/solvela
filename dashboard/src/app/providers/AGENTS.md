<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# providers

## Purpose
Client-side React providers wrapped around the app. Currently only the theme provider (light/dark/system). Anything that needs browser-side context goes here.

## Key Files
| File | Description |
|------|-------------|
| `theme-provider.tsx` | Wraps `next-themes` `ThemeProvider` with the project's defaults |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- Every file here is a client component (`"use client"` at the top).
- Keep providers tree-shakeable — don't import server-only code.
- Wire new providers into `app/layout.tsx` inside the server layout; keep the provider itself focused on one concern.

### Testing Requirements
```bash
npm --prefix dashboard test
```

### Common Patterns
- Re-export common hooks alongside the provider so consumers don't import two modules.

## Dependencies

### Internal
- Referenced from `app/layout.tsx`.

### External
- `next-themes` (or equivalent).

<!-- MANUAL: -->
