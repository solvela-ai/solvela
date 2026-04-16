import { createMDX } from 'fumadocs-mdx/next';
import { fileURLToPath } from 'node:url';
import { dirname } from 'node:path';

const __dirname = dirname(fileURLToPath(import.meta.url));

const config = {
  reactStrictMode: true,
  turbopack: {
    root: __dirname,
  },
};

const withMDX = createMDX();

export default withMDX(config);
