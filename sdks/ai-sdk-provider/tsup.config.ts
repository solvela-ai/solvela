import { defineConfig } from 'tsup';

export default defineConfig({
  entry: {
    index: 'src/index.ts',
    'adapters/local': 'src/adapters/local.ts',
  },
  format: ['esm'],
  // The provider runs server-side: it signs Solana transactions and reads
  // process env. signer-core (a transitive dep) imports from node:crypto,
  // so the bundler must treat node: specifiers as built-in externals.
  // platform:'node' alone wasn't enough in tsup 8.5 — esbuild still
  // tried to resolve node:crypto on disk. The explicit external regex
  // is the belt-and-suspenders fix.
  platform: 'node',
  external: [/^node:/],
  dts: true,
  splitting: false,
  sourcemap: true,
  clean: true,
  outDir: 'dist',
});
