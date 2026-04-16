<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# public

## Purpose
Static assets served at the web root (`/`). Next.js copies everything here into the deployed bundle verbatim. Use for images, fonts, OG/social previews, favicons, and anything that should be cached at the CDN edge without going through the build pipeline.

## For AI Agents

### Working In This Directory
- Reference these files with absolute paths (`<img src="/logo.svg" />`) — no imports.
- Prefer `next/image` + colocated images under `src/` for anything processed (optimization, responsive sizing); use `public/` for static assets that Next shouldn't touch.
- Don't commit large unoptimized images here — squeeze them first.

### Testing Requirements
Smoke test: hit the asset URL in the browser after `npm --prefix dashboard run dev` and confirm it loads.

### Common Patterns
- SVG for logos/icons, WebP/AVIF for raster where feasible.
- Meta-image / OG-image conventions under `/` so `next/og` or meta tags can reference them directly.

## Dependencies
_(pure static)_

<!-- MANUAL: -->
