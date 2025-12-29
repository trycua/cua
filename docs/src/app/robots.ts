import { MetadataRoute } from 'next';

export default function robots(): MetadataRoute.Robots {
  return {
    rules: {
      userAgent: '*',
      allow: ['/', '/llms.txt'],
      disallow: [],
    },
    sitemap: 'https://cua.ai/docs/sitemap.xml',
    host: 'https://cua.ai',
  };
}
