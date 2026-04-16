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
        // docs.solvela.ai/ → /docs
        {
          source: '/',
          has: [{ type: 'host', value: 'docs.solvela.ai' }],
          destination: '/docs',
        },
        // docs.solvela.ai/:path* → /docs/:path* (excludes /_next and /api)
        {
          source: '/:path((?!_next|api).*)',
          has: [{ type: 'host', value: 'docs.solvela.ai' }],
          destination: '/docs/:path',
        },
        // app.solvela.ai/ → /dashboard
        {
          source: '/',
          has: [{ type: 'host', value: 'app.solvela.ai' }],
          destination: '/dashboard',
        },
        // app.solvela.ai/:path* → /dashboard/:path* (excludes /_next and /api)
        {
          source: '/:path((?!_next|api).*)',
          has: [{ type: 'host', value: 'app.solvela.ai' }],
          destination: '/dashboard/:path',
        },
      ],
    };
  },
};

const withMDX = createMDX();

export default withMDX(config);
