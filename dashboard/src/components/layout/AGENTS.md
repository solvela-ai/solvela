<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# layout

## Purpose
The dashboard app shell. `Shell` composes `Sidebar` + `Topbar` around page content. Add / rename dashboard pages by editing `sidebar.tsx`; change global chrome by editing `topbar.tsx` or `shell.tsx`.

## Key Files
| File | Description |
|------|-------------|
| `shell.tsx` | Root shell — flex layout that places sidebar + topbar + main content |
| `sidebar.tsx` | Collapsible left sidebar with nav links (Overview/Usage/Models/Wallet/Settings) |
| `topbar.tsx` | Top bar — breadcrumbs, theme switcher, wallet indicator |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- When you add a dashboard page under `app/dashboard/*`, add a matching entry in `sidebar.tsx`.
- `Shell` is a server component; `Sidebar` / `Topbar` are client components because they handle interactivity.
- Theme switching lives in `topbar.tsx` — keep logic there, not scattered across pages.

### Testing Requirements
```bash
npm --prefix dashboard test
npm --prefix dashboard run dev
```

### Common Patterns
- Use semantic HTML (`<nav>`, `<aside>`, `<main>`) for accessibility.
- Keyboard-reachable nav items; focus styles from Tailwind's `focus-visible` utilities.

## Dependencies

### Internal
- `@/components/ui`, `@/lib/icons`.

### External
- Next.js `<Link>`, Tailwind.

<!-- MANUAL: -->
