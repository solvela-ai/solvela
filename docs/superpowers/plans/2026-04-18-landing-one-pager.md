# Solvela One-Page Landing — Implementation Plan

> **Date**: 2026-04-18
> **Host**: `solvela.ai/` (middleware pass-through → `app/page.tsx`)
> **Scope**: Replace the `/` → `/docs` redirect with a bespoke marketing one-pager that *looks and behaves like the product*. Docs and app dashboards are unchanged.

---

## 1. Thesis

**Headline:** "Trustless escrow for agent payments. Pay only for what your agent receives."
**Category line:** "Solana-native x402 gateway for AI agents."

Agent-first copy throughout — every example is something an agent does, not something a developer reads. Escrow is the lead differentiator (per `docs/strategy/2026-04-17-competitive-analysis.md`); x402 and Solana are the substrate.

## 2. Visual Direction

One-page landing that *is* a dashboard frame. Editorial typography + brutalist grid + one signature animation. Inspiration:
- **Browser Cash app** (screenshots in `pics/`) — framed panels, uppercase mono labels, terminal cards, accent CTA, sidebar-chromed layout
- **Browser Cash landing** — one isometric illustration moment, bold editorial headlines
- **CalArts / figr editorial polish** — restraint over density, a few bold type moves

**Palette:** existing solvela warm-charcoal + salmon. No new colors. (`#262624` bg, `#DEDCD1` text, `#FE8181` salmon, `#C8A240` gold borders.)

**Typography:** existing stack — Archivo (display headlines), Source Serif 4 (large metrics), DM Sans (body), JetBrains Mono (eyebrows, labels). All four are already loaded in `src/app/layout.tsx`.

## 3. Composition (6 sections + chrome)

```
┌─ Top strip ─ thin mono bar: SOLVELA · 0.5.0 · MAINNET · [docs] [app] [github]
├─ Hero panel (full-bleed, bg-grid, left-align)
│  EYEBROW  | SOL × x402 · mainnet
│  HEADLINE | Trustless escrow for
│           | agent payments.
│  SUB      | Pay only for what your agent receives.
│  INLINE   | [Terminal card → live 402 handshake (signature animation)]
│  CTAs     | [ start building → ]  [ view docs ]
├─ Metric row (4 tiles in a row)
│  99.98% uptime · 38ms p50 · 26 models · 5% flat fee
├─ Escrow hero panel (the differentiator — centered, single SVG isometric)
│  Editorial copy + one isometric CSS diagram: wallet → escrow PDA → provider → claim/refund
├─ Provider + SDK row (two framed panels side-by-side)
│  Left: 5 providers, mono labels, status dots
│  Right: Terminal card with SDK tabs (TS / Py / Go / Rust / MCP)
├─ CTA panel (terminal card with a real curl)
│  Single bordered panel, one-line headline, copyable curl, salmon primary button
└─ Footer (mono columns: product · docs · legal · on-chain addresses)
```

No route group. Single `app/page.tsx` composes named components from `src/components/landing/`.

## 4. Signature Animation — Hero Terminal Morph

One effect, not four. The hero terminal card plays a ~5-second autoplay loop (respects `prefers-reduced-motion`):

1. **t=0.0s** — empty terminal, cursor blink
2. **t=0.3s** — curl command types in (type-on animation, 2.5s)
3. **t=2.8s** — 402 response fades in with syntax highlight, shows cost breakdown
4. **t=3.6s** — payment payload "signs" (green status dot pulse)
5. **t=4.2s** — LLM response streams token-by-token
6. **t=5.0s** — pause; loop

This *demonstrates the product* while being the animation. No SVG-draw, no fake cursor, no skeleton morph. The rest of the page uses existing `.animate-fade-in-up` + stagger on scroll into viewport — that's ambient, not signature.

Implementation: JS-driven with `setInterval`/`requestAnimationFrame` in a client component `<HeroTerminal client />`. Types via character-by-character append to a `<pre>`. No external libs.

## 5. File Plan

```
dashboard/src/
├── app/
│   └── page.tsx                      (replace redirect — server component entry)
├── components/
│   └── landing/
│       ├── landing-chrome.tsx        (top strip + footer)
│       ├── hero-panel.tsx            (server + client child)
│       ├── hero-terminal.tsx         (client, signature animation)
│       ├── metric-row.tsx            (server, with client count-up tiles)
│       ├── count-up.tsx              (client, IO-triggered)
│       ├── escrow-panel.tsx          (server with CSS isometric diagram)
│       ├── provider-sdk-row.tsx      (server wrapper)
│       ├── provider-grid.tsx         (server)
│       ├── sdk-tabs.tsx              (client, tab state)
│       └── cta-panel.tsx             (server + client copy button)
└── app/
    └── globals.css                   (append: type-on keyframe, isometric helpers)
```

No new npm deps. Lucide already covers icons.

## 6. Tokens Reused (no new CSS variables introduced)

- `.terminal-card` + `.terminal-card-titlebar` + `.terminal-card-dots` + `.terminal-card-screen`
- `.bg-grid` / `.bg-grid-dense`
- `.metric-xl`, `.metric-lg`, `.metric-md`
- `.eyebrow`, `.section-header`, `.divider-fade`
- `.animate-fade-in-up`, `.delay-1..5`, `.card-nested`, `.card-surface`
- Fonts: `var(--font-display)`, `var(--font-serif)`, `var(--font-sans)`, `var(--font-mono)`
- Colors: `--accent-salmon`, `--foreground`, `--heading-color`, `--muted-foreground`, `--card`, `--border`, `--sidebar-bg`, `--popover`

## 7. Accessibility + Perf

- All motion gated by `@media (prefers-reduced-motion: reduce)` — already in `globals.css`
- Heading order: `h1` once (hero headline), `h2` per section, no heading skips
- Color contrast: existing palette passes AA; verify salmon on charcoal (≥4.5:1 for body, ≥3:1 for large)
- Static-first: hero page is server-rendered; only the terminal, count-up, SDK tabs, copy button are `'use client'`
- No third-party analytics, fonts, or scripts

## 8. Out of Scope

- New `/start` or pricing routes — CTA links to `docs.solvela.ai/docs/quickstart`
- Light-mode variant (dark-only per existing theme decision)
- Real API reads — hero numbers are compile-time constants
- Component-library extraction (reuse `terminal-card` CSS; don't factor out React primitives yet)

## 9. Verification

- `cd dashboard && npm run dev` → `http://localhost:3000/` renders landing
- `cd dashboard && npm run build` — clean build, no type errors
- `cd dashboard && npm run lint` — clean
- Visual: hero animation loops smoothly in Chrome; stagger appears on scroll; reduced-motion kills all animation
- Middleware paths untouched: `docs.solvela.ai`, `app.solvela.ai` still rewrite correctly

## 10. Risks and Mitigations

| Risk | Mitigation |
|---|---|
| Terminal animation feels gimmicky | Keep one loop, 5s, ease-out; stop after 3 cycles; user can resume via click |
| Replacing `/` redirect breaks docs SEO | `/docs` is the canonical URL on `docs.solvela.ai`; landing at apex is correct |
| Bundle size grows from client islands | All islands are <3KB gzipped; no new deps; measurable via `next build` |
| Copy tone slips into "developer" voice | Reviewed by `designer` agent for agent-first consistency |

## 11. Review Gates

Dispatch in parallel before shipping:
- `oh-my-claudecode:designer` — composition, typographic hierarchy, animation budget, color discipline
- `oh-my-claudecode:critic` — catches missing items, verifies thesis alignment with competitive analysis

Flag to user only if review surfaces something they'd reverse.
