import { defineConfig, defineDocs, frontmatterSchema, metaSchema } from 'fumadocs-mdx/config';
import { z } from 'zod';

// Extended frontmatter schema
const extendedFrontmatter = frontmatterSchema.extend({
  macos: z.boolean().optional(),
  windows: z.boolean().optional(),
  linux: z.boolean().optional(),
  pypi: z.string().optional(),
  npm: z.string().optional(),
  github: z.array(z.string()).optional(),
  keywords: z.array(z.string()).optional(),
  // The docs.cua.ai landing: the home page's hero, path blurb and examples,
  // and each product Overview's card blurb and call to action.
  landing: z
    .object({
      hero: z.string().optional(),
      path: z.string().optional(),
      products: z.array(z.string()).optional(),
      summary: z.string().optional(),
      cta: z.string().optional(),
      examples: z
        .array(
          // Media is an image, or a video with its poster. The title may
          // bold a phrase with **markdown**.
          z.object({
            title: z.string(),
            description: z.string().optional(),
            href: z.string(),
            cta: z.string().optional(),
            image: z.string().optional(),
            video: z.string().optional(),
            poster: z.string().optional(),
            alt: z.string().optional(),
            links: z.array(z.object({ title: z.string(), href: z.string() })).optional(),
          })
        )
        .optional(),
    })
    .optional(),
});

// Single docs collection
export const docs = defineDocs({
  dir: 'content/docs',
  docs: {
    schema: extendedFrontmatter,
    // Compile each page on demand. Eager mode puts every MDX module (generated
    // reference included) in one import graph, so the dev server compiled all
    // of them on the first request and grew past 10 GB.
    async: true,
  },
  meta: {
    schema: metaSchema,
  },
});

export default defineConfig({
  mdxOptions: {
    rehypeCodeOptions: {
      fallbackLanguage: 'text',
      themes: {
        light: 'github-light',
        dark: 'github-dark',
      },
    },
  },
});
