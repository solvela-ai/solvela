import { defineDocs, defineConfig } from 'fumadocs-mdx/config'
import { rehypeCode } from 'fumadocs-core/mdx-plugins'
import solvelaDark from './src/lib/shiki/solvela-dark.json' with { type: 'json' }
import solvelaLight from './src/lib/shiki/solvela-light.json' with { type: 'json' }

export const docs = defineDocs({
  dir: 'content/docs',
})

export default defineConfig({
  mdxOptions: {
    rehypePlugins: [
      [
        rehypeCode,
        {
          themes: {
            light: solvelaLight,
            dark: solvelaDark,
          },
        },
      ],
    ],
  },
})
