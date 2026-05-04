# Solvela Brand Assets

Canonical brand and avatar assets for the Solvela project. Source-of-truth for everything visual that represents Solvela on GitHub, npm, PyPI, crates.io, the dashboard, and social channels.

## Design system

- **Primary mark**: hexagonal node frame around a stylized "S" — the hex evokes the on-chain / network nature of x402, and the six vertex nodes signal nodes-on-a-graph.
- **Master gradient**: violet `#7C3AED` → cyan `#22D3EE` (top-left to bottom-right).
- **Surface**: deep navy `#08081A`, rounded corner radius `48` on a 400×400 viewBox (12% of the side).
- **SDK badge color** matches each ecosystem's canonical brand color so the variant is recognizable at a glance.

| Variant | Hex frame color | Badge | Repo |
|---|---|---|---|
| Master | violet→cyan gradient | — | `solvela-ai/solvela` + org avatar |
| Python | `#3776AB` (Python blue) | `py` | `solvela-ai/solvela-python` |
| TypeScript | `#3178C6` (TS blue) | `ts` | `solvela-ai/solvela-ts` |
| Go | `#00ADD8` (Go teal) | `go` | `solvela-ai/solvela-go` |
| Rust client | `#CE422B` (Rust orange) | `rs` | `solvela-ai/solvela-client` |

## File layout

```
brand/
├── README.md            # this file
├── rasterize.mjs        # SVG → PNG (multi-size) generator
└── avatars/
    ├── solvela.svg              # master mark (canonical)
    ├── solvela-python.svg       # SDK variant
    ├── solvela-ts.svg
    ├── solvela-go.svg
    ├── solvela-client.svg
    └── png/                     # generated; do not hand-edit
        ├── solvela-{16,32,64,128,256,500}.png
        ├── solvela-python-{16,32,64,128,256,500}.png
        ├── solvela-ts-{16,32,64,128,256,500}.png
        ├── solvela-go-{16,32,64,128,256,500}.png
        └── solvela-client-{16,32,64,128,256,500}.png
```

## Regenerating PNGs

The PNGs in `avatars/png/` are generated from the SVGs using `sharp` (which is already a transitive dependency of the dashboard). To regenerate:

```bash
node brand/rasterize.mjs
```

Sizes produced (each variant): 16, 32, 64, 128, 256, 500.

| Size | Use |
|---|---|
| 500 | GitHub org/repo avatar, npm/PyPI/crates.io profile |
| 256 | High-DPI npm display, OG card thumbnails |
| 128 | Inline README mark |
| 64 | Inline icon in docs |
| 32 | Favicon |
| 16 | Favicon (legacy) |

## Upload checklist

When seeding the GitHub org for the first time, or refreshing branding:

### Org-level (`solvela-ai`)
- [ ] **Avatar**: upload `avatars/png/solvela-500.png` at https://github.com/organizations/solvela-ai/settings/profile
- [ ] **Description**: `Solana-native x402 LLM gateway. Stablecoin payments for AI agents.`
- [ ] **URL**: `https://solvela.ai`
- [ ] **Email**: `partnerships@solvela.ai` (or `kd@sky64.io` until that alias is set up)
- [ ] **Location**: city/country (helps Superteam regional grant gating)
- [ ] **Twitter/X**: org handle if/when one exists
- [ ] **Pinned repos** in this order: `solvela`, `solvela-python`, `solvela-ts`, `solvela-go`, `solvela-client`

### Per-repo

For each repo, in repo Settings → General → upload the matching avatar, then in the repo sidebar (the gear icon next to "About") set:

| Repo | Avatar | About description | Topics |
|---|---|---|---|
| `solvela-ai/solvela` | `solvela-500.png` | Solana-native x402 LLM gateway. Stablecoin payments for AI agents. | `solana`, `x402`, `llm-gateway`, `ai-agents`, `stablecoins`, `usdc`, `mcp`, `a2a`, `rust` |
| `solvela-ai/solvela-python` | `solvela-python-500.png` | Python SDK for Solvela. | `solvela`, `x402`, `llm`, `python`, `ai-agents` |
| `solvela-ai/solvela-ts` | `solvela-ts-500.png` | TypeScript SDK for Solvela. | `solvela`, `x402`, `llm`, `typescript`, `ai-agents` |
| `solvela-ai/solvela-go` | `solvela-go-500.png` | Go SDK for Solvela. | `solvela`, `x402`, `llm`, `golang`, `ai-agents` |
| `solvela-ai/solvela-client` | `solvela-client-500.png` | Rust client for x402 stablecoin payments on Solana. | `solana`, `x402`, `rust`, `usdc`, `payments` |

The `.github` repo (org profile README) does not need an avatar.

### Package registries

Each registry has its own avatar slot. Use the same per-language PNG used on the GitHub repo:

- **PyPI** (`solvela-python` package owner): https://pypi.org/manage/account/profile/
- **npm** (`@solvela-ai` org or package): https://www.npmjs.com/settings/solvela-ai/profile
- **crates.io** (per-crate): no avatar slot, but the README banner is the next-best thing — use `solvela-256.png` inline.

### Dashboard / favicons

- `dashboard/public/favicon.ico` — multi-size ICO built from `solvela-{16,32,64}.png` (use https://realfavicongenerator.net or `magick convert` if available)
- `dashboard/public/apple-touch-icon.png` — `solvela-256.png` (rename to `apple-touch-icon.png`)
- `dashboard/public/og-default.png` — needs a separate 1200×630 OG variant; not produced by `rasterize.mjs` because OG cards have horizontal layout. Build separately when you need it.

## Design rules

If touching the SVGs:

1. **Keep the hex frame.** It's the most recognizable element. Don't replace with a circle, square, or other shape.
2. **Keep the violet→cyan gradient on the master S.** Variants recolor only the hex frame and badge, never the S.
3. **Keep the dark navy `#08081A` background.** It's intentionally near-black but not pure black, which reads better on both light and dark GitHub themes.
4. **No drop shadows on the canvas.** The internal `glow` filter is the only effect — it's narrow and intentional. Don't stack shadows.
5. **Preserve the rounded corners on the rasterized PNG.** The `rx="48"` on the background `<rect>` is what GitHub crops to a circle cleanly without the corners getting clipped awkwardly.

## What not to use

- `assets/Gemini_Generated_Image_*.png` — concept exploration only, not production assets. The wordmark direction in those images conflicts with this brand system. They are kept in the repo for historical reference but should not be uploaded anywhere public.

## Trademark

`SOLVELA` is the subject of a planned USPTO trademark application (classes 9 + 42). See `docs/trademark/SOLVELA-USPTO-application.md`. The marks in this directory are © Solvela Contributors and are licensed under MIT for code-form uses; the **wordmark and stylized S logo** are not licensed under MIT and may not be used to represent products that are not Solvela. See the project `README.md` Licensing section.
