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
  // Without platform:'node' tsup tries to resolve node:crypto from
  // node_modules and fails.
  platform: 'node',
  dts: true,
  splitting: false,
  sourcemap: true,
  clean: true,
  outDir: 'dist',
});
