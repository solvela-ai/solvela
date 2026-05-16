import { createMDX } from 'fumadocs-mdx/next';
import { fileURLToPath } from 'node:url';
import { dirname } from 'node:path';

const __dirname = dirname(fileURLToPath(import.meta.url));

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
};

const withMDX = createMDX();

export default withMDX(config);
