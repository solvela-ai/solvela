import { createMDX } from 'fumadocs-mdx/next';
import { fileURLToPath } from 'node:url';
import { dirname } from 'node:path';

const __dirname = dirname(fileURLToPath(import.meta.url));

const config = {
  reactStrictMode: true,
  turbopack: {
    root: __dirname,
  },
  async rewrites() {
    return {
      beforeFiles: [
        // docs.solvela.ai: pass-through for Next.js internals
        { source: '/_next/:path*', has: [{ type: 'host', value: 'docs.solvela.ai' }], destination: '/_next/:path*' },
        { source: '/api/:path*',   has: [{ type: 'host', value: 'docs.solvela.ai' }], destination: '/api/:path*' },
        // docs.solvela.ai: everything else → /docs/* (including empty for root)
        { source: '/:path*',       has: [{ type: 'host', value: 'docs.solvela.ai' }], destination: '/docs/:path*' },

        // app.solvela.ai: pass-through for Next.js internals
        { source: '/_next/:path*', has: [{ type: 'host', value: 'app.solvela.ai' }], destination: '/_next/:path*' },
        { source: '/api/:path*',   has: [{ type: 'host', value: 'app.solvela.ai' }], destination: '/api/:path*' },
        // app.solvela.ai: everything else → /dashboard/*
        { source: '/:path*',       has: [{ type: 'host', value: 'app.solvela.ai' }], destination: '/dashboard/:path*' },
      ],
    };
  },
};

const withMDX = createMDX();

export default withMDX(config);
