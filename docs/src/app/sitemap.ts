import { MetadataRoute } from 'next';
import { source } from '@/lib/source';

export default function sitemap(): MetadataRoute.Sitemap {
  const baseUrl = 'https://cua.ai';

  // Get all pages from fumadocs source
  const pages = source.getPages();

  // Map pages to sitemap entries with /docs prefix
  const docPages = pages.map((page) => {
    // Ensure URL starts with /docs
    const url = page.url.startsWith('/docs') ? page.url : `/docs${page.url}`;

    return {
      url: `${baseUrl}${url}`,
      lastModified: page.data.lastModified
        ? new Date(page.data.lastModified)
        : undefined,
      changeFrequency: 'weekly' as const,
      priority: url === '/docs' ? 1.0 : 0.8,
    };
  });

  // Add main docs page if not included
  const mainDocsPage = {
    url: `${baseUrl}/docs`,
    lastModified: docPages[0]?.lastModified,
    changeFrequency: 'weekly' as const,
    priority: 1.0,
  };

  return [mainDocsPage, ...docPages];
}
