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
});

// Single docs collection
export const docs = defineDocs({
  dir: 'content/docs',
  docs: {
    schema: extendedFrontmatter,
  },
  meta: {
    schema: metaSchema,
  },
});

export default defineConfig({
  mdxOptions: {
    // MDX options
  },
});
