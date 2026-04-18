import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    environment: 'node',
    coverage: {
      provider: 'v8',
      reporter: ['text', 'lcov'],
      exclude: [
        'scripts/**',
        'dist/**',
        'src/generated/**',
        'src/index.ts',
      ],
    },
    include: ['tests/**/*.test.ts'],
  },
});
