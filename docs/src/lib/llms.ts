import { remark } from 'remark';
import remarkGfm from 'remark-gfm';
import remarkMdx from 'remark-mdx';
import { remarkInclude } from 'fumadocs-mdx/config';
import { source } from '@/lib/source';
import type { InferPageType } from 'fumadocs-core/source';

const processor = remark()
  .use(remarkMdx)
  // needed for Fumadocs MDX
  .use(remarkInclude)
  .use(remarkGfm);

export async function getLLMText(page: InferPageType<typeof source>) {
  const pageData = page.data as any;
  const filePath = pageData._file?.absolutePath;
  const content = pageData.content || pageData.body || '';

  let processed;
  if (filePath && typeof content === 'string') {
    processed = await processor.process({ path: filePath, value: content });
  } else if (typeof content === 'string') {
    processed = await processor.process(content);
  } else {
    // Handle case where content is not available
    processed = { value: '' };
  }

  return `# ${page.data.title}
URL: ${page.url}

${page.data.description || ''}

${processed.value}`;
}
