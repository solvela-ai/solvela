import { createMDX } from 'fumadocs-mdx/next';
import { fileURLToPath } from 'node:url';
import { dirname } from 'node:path';

const __dirname = dirname(fileURLToPath(import.meta.url));

// ─── Security headers (CSP + hardening) ───────────────────────────────────────
// Added 2026-06-08 (security audit Scope 4). Vercel injects no security headers
// of its own, so we own all of them here.
//
// Defense-in-depth posture: the CSP `connect-src` limits where an injected script
// could exfiltrate the localStorage org API key to. It is a BACKSTOP, not the root
// fix — the real remediation is moving that key behind a server-side BFF proxy
// (the pattern already used for GATEWAY_ADMIN_KEY); tracked separately.
//
// `'unsafe-inline'` is deliberate, documented compatibility debt: Next's App
// Router hydration/flight scripts + next-themes (script-src) and Recharts SVG
// style attributes + Shiki token colors + next/font/Tailwind v4 (style-src) all
// emit inline content that per-request nonces cannot cover — style *attributes*
// ignore nonces, and a nonce CSP would force every route to fully dynamic
// rendering (no static/ISR/PPR). The real security value here is in connect-src /
// frame-ancestors / object-src / base-uri, which hold even with 'unsafe-inline'.
const isDev = process.env.NODE_ENV === 'development';

// Browser-side fetch only ever targets the gateway (src/lib/api.ts). Derive its
// origin from the public env var; fall back to the per-environment default.
const gatewayOrigin = (() => {
  const raw = process.env.NEXT_PUBLIC_GATEWAY_URL;
  if (raw) {
    try {
      return new URL(raw).origin;
    } catch {
      // malformed env → fall through to the environment default
    }
  }
  return isDev ? 'http://localhost:8402' : 'https://api.solvela.ai';
})();

const csp = [
  `default-src 'self'`,
  `script-src 'self' 'unsafe-inline'${isDev ? " 'unsafe-eval'" : ''}`,
  `style-src 'self' 'unsafe-inline'`,
  `img-src 'self' data:`,
  `font-src 'self'`,
  `connect-src 'self' ${gatewayOrigin}${isDev ? ' ws: wss:' : ''}`,
  `frame-ancestors 'none'`,
  `object-src 'none'`,
  `base-uri 'self'`,
  `form-action 'self'`,
  // Auto-upgrade stray http subresources in prod only (would break the
  // http://localhost gateway in dev).
  ...(isDev ? [] : [`upgrade-insecure-requests`]),
].join('; ');

const securityHeaders = [
  { key: 'Content-Security-Policy', value: csp },
  { key: 'X-Content-Type-Options', value: 'nosniff' },
  { key: 'Referrer-Policy', value: 'strict-origin-when-cross-origin' },
  { key: 'X-Frame-Options', value: 'DENY' },
  {
    key: 'Permissions-Policy',
    value: 'camera=(), microphone=(), geolocation=(), browsing-topics=()',
  },
  // HSTS in production only (ignored over http anyway). No `preload` — that is a
  // hard-to-reverse all-subdomain commitment; revisit deliberately.
  ...(isDev
    ? []
    : [
        {
          key: 'Strict-Transport-Security',
          value: 'max-age=63072000; includeSubDomains',
        },
      ]),
];

const config = {
  reactStrictMode: true,
  turbopack: {
    root: __dirname,
  },
  // /pricing → internal docs page redirect.
  //
  // The apex `solvela.ai/pricing` URL has never existed (no `pricing/`
  // route under `dashboard/src/app/`), but external links — twitter
  // bios, blog posts, prior enterprise-docs CTAs — used to point at
  // it. Those references in `dashboard/content/docs/` are gone (see
  // PR #270 + the Next.js 16 migration), but anything cached
  // externally still 404s. Soft-handoff to the existing internal
  // pricing docs page (`/docs/concepts/pricing`) — covers gateway-
  // fee math, the model-pricing table, and the `/pricing` API
  // endpoint — until/unless a dedicated apex marketing page is
  // built. `permanent: true` emits HTTP 308 so browsers, proxies,
  // and search engines can cache the handoff. See issue #82.
  redirects: async () => [
    {
      source: '/pricing',
      destination: '/docs/concepts/pricing',
      permanent: true,
    },
  ],
  headers: async () => [
    {
      source: '/:path*',
      headers: securityHeaders,
    },
  ],
};

const withMDX = createMDX();

export default withMDX(config);
