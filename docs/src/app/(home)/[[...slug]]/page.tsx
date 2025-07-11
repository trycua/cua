import { getApiVersions, source } from '@/lib/source';
import { getMDXComponents } from '@/mdx-components';
import { buttonVariants } from 'fumadocs-ui/components/ui/button';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from 'fumadocs-ui/components/ui/popover';
import { createRelativeLink } from 'fumadocs-ui/mdx';
import {
  DocsBody,
  DocsDescription,
  DocsPage,
  DocsTitle,
} from 'fumadocs-ui/page';
import { cn } from 'fumadocs-ui/utils/cn';
import { ChevronDown, CodeXml, ExternalLink } from 'lucide-react';
import type { Metadata } from 'next';
import Link from 'next/link';
import { notFound, redirect } from 'next/navigation';

export default async function Page(props: {
  params: Promise<{ slug?: string[] }>;
}) {
  const params = await props.params;
  const slug = params.slug || [];
  const page = source.getPage(slug);
  if (!page) redirect('/home');

  // Detect if this is an API reference page: /api/[section] or /api/[section]/[version]
  let apiSection: string | null = null;
  let apiVersionSlug: string[] = [];
  if (slug[0] === 'api' && slug.length >= 2) {
    apiSection = slug[1];
    if (slug.length > 2) {
      apiVersionSlug = slug.slice(2);
    }
  }

  let versionItems: { label: string; slug: string[] }[] = [];
  if (apiSection) {
    versionItems = await getApiVersions(apiSection);
  }

  const macos = page.data.macos;
  const windows = page.data.windows;
  const linux = page.data.linux;
  const pypi = page.data.pypi;
  const npm = page.data.npm;
  const github = page.data.github;

  const MDXContent = page.data.body;

  // Platform icons component
  const PlatformIcons = () => {
    const hasAnyPlatform = macos || windows || linux;
    if (!hasAnyPlatform && !pypi) return null;

    return (
      <div className="flex flex-col gap-2">
        {hasAnyPlatform && (
          <div className="flex flex-row gap-2 items-left dark:text-neutral-400">
            {windows && (
              <svg
                xmlns="http://www.w3.org/2000/svg"
                fill="currentColor"
                className="h-5"
                viewBox="0 0 448 512">
                <title>Windows</title>
                <path d="M0 93.7l183.6-25.3v177.4H0V93.7zm0 324.6l183.6 25.3V268.4H0v149.9zm203.8 28L448 480V268.4H203.8v177.9zm0-380.6v180.1H448V32L203.8 65.7z" />
              </svg>
            )}
            {macos && (
              <svg
                xmlns="http://www.w3.org/2000/svg"
                fill="currentColor"
                className="h-5"
                viewBox="0 0 384 512">
                <title>macOS</title>
                <path d="M318.7 268.7c-.2-36.7 16.4-64.4 50-84.8-18.8-26.9-47.2-41.7-84.7-44.6-35.5-2.8-74.3 20.7-88.5 20.7-15 0-49.4-19.7-76.4-19.7C63.3 141.2 4 184.8 4 273.5q0 39.3 14.4 81.2c12.8 36.7 59 126.7 107.2 125.2 25.2-.6 43-17.9 75.8-17.9 31.8 0 48.3 17.9 76.4 17.9 48.6-.7 90.4-82.5 102.6-119.3-65.2-30.7-61.7-90-61.7-91.9zm-56.6-164.2c27.3-32.4 24.8-61.9 24-72.5-24.1 1.4-52 16.4-67.9 34.9-17.5 19.8-27.8 44.3-25.6 71.9 26.1 2 49.9-11.4 69.5-34.3z" />
              </svg>
            )}
            {linux && (
              <svg
                xmlns="http://www.w3.org/2000/svg"
                fill="currentColor"
                className="h-5"
                viewBox="0 0 448 512">
                <title>Linux</title>
                <path d="M220.8 123.3c1 .5 1.8 1.7 3 1.7 1.1 0 2.8-.4 2.9-1.5 .2-1.4-1.9-2.3-3.2-2.9-1.7-.7-3.9-1-5.5-.1-.4 .2-.8 .7-.6 1.1 .3 1.3 2.3 1.1 3.4 1.7zm-21.9 1.7c1.2 0 2-1.2 3-1.7 1.1-.6 3.1-.4 3.5-1.6 .2-.4-.2-.9-.6-1.1-1.6-.9-3.8-.6-5.5 .1-1.3 .6-3.4 1.5-3.2 2.9 .1 1 1.8 1.5 2.8 1.4zM420 403.8c-3.6-4-5.3-11.6-7.2-19.7-1.8-8.1-3.9-16.8-10.5-22.4-1.3-1.1-2.6-2.1-4-2.9-1.3-.8-2.7-1.5-4.1-2 9.2-27.3 5.6-54.5-3.7-79.1-11.4-30.1-31.3-56.4-46.5-74.4-17.1-21.5-33.7-41.9-33.4-72C311.1 85.4 315.7 .1 234.8 0 132.4-.2 158 103.4 156.9 135.2c-1.7 23.4-6.4 41.8-22.5 64.7-18.9 22.5-45.5 58.8-58.1 96.7-6 17.9-8.8 36.1-6.2 53.3-6.5 5.8-11.4 14.7-16.6 20.2-4.2 4.3-10.3 5.9-17 8.3s-14 6-18.5 14.5c-2.1 3.9-2.8 8.1-2.8 12.4 0 3.9 .6 7.9 1.2 11.8 1.2 8.1 2.5 15.7 .8 20.8-5.2 14.4-5.9 24.4-2.2 31.7 3.8 7.3 11.4 10.5 20.1 12.3 17.3 3.6 40.8 2.7 59.3 12.5 19.8 10.4 39.9 14.1 55.9 10.4 11.6-2.6 21.1-9.6 25.9-20.2 12.5-.1 26.3-5.4 48.3-6.6 14.9-1.2 33.6 5.3 55.1 4.1 .6 2.3 1.4 4.6 2.5 6.7v.1c8.3 16.7 23.8 24.3 40.3 23 16.6-1.3 34.1-11 48.3-27.9 13.6-16.4 36-23.2 50.9-32.2 7.4-4.5 13.4-10.1 13.9-18.3 .4-8.2-4.4-17.3-15.5-29.7zM223.7 87.3c9.8-22.2 34.2-21.8 44-.4 6.5 14.2 3.6 30.9-4.3 40.4-1.6-.8-5.9-2.6-12.6-4.9 1.1-1.2 3.1-2.7 3.9-4.6 4.8-11.8-.2-27-9.1-27.3-7.3-.5-13.9 10.8-11.8 23-4.1-2-9.4-3.5-13-4.4-1-6.9-.3-14.6 2.9-21.8zM183 75.8c10.1 0 20.8 14.2 19.1 33.5-3.5 1-7.1 2.5-10.2 4.6 1.2-8.9-3.3-20.1-9.6-19.6-8.4 .7-9.8 21.2-1.8 28.1 1 .8 1.9-.2-5.9 5.5-15.6-14.6-10.5-52.1 8.4-52.1zm-13.6 60.7c6.2-4.6 13.6-10 14.1-10.5 4.7-4.4 13.5-14.2 27.9-14.2 7.1 0 15.6 2.3 25.9 8.9 6.3 4.1 11.3 4.4 22.6 9.3 8.4 3.5 13.7 9.7 10.5 18.2-2.6 7.1-11 14.4-22.7 18.1-11.1 3.6-19.8 16-38.2 14.9-3.9-.2-7-1-9.6-2.1-8-3.5-12.2-10.4-20-15-8.6-4.8-13.2-10.4-14.7-15.3-1.4-4.9 0-9 4.2-12.3zm3.3 334c-2.7 35.1-43.9 34.4-75.3 18-29.9-15.8-68.6-6.5-76.5-21.9-2.4-4.7-2.4-12.7 2.6-26.4v-.2c2.4-7.6 .6-16-.6-23.9-1.2-7.8-1.8-15 .9-20 3.5-6.7 8.5-9.1 14.8-11.3 10.3-3.7 11.8-3.4 19.6-9.9 5.5-5.7 9.5-12.9 14.3-18 5.1-5.5 10-8.1 17.7-6.9 8.1 1.2 15.1 6.8 21.9 16l19.6 35.6c9.5 19.9 43.1 48.4 41 68.9zm-1.4-25.9c-4.1-6.6-9.6-13.6-14.4-19.6 7.1 0 14.2-2.2 16.7-8.9 2.3-6.2 0-14.9-7.4-24.9-13.5-18.2-38.3-32.5-38.3-32.5-13.5-8.4-21.1-18.7-24.6-29.9s-3-23.3-.3-35.2c5.2-22.9 18.6-45.2 27.2-59.2 2.3-1.7 .8 3.2-8.7 20.8-8.5 16.1-24.4 53.3-2.6 82.4 .6-20.7 5.5-41.8 13.8-61.5 12-27.4 37.3-74.9 39.3-112.7 1.1 .8 4.6 3.2 6.2 4.1 4.6 2.7 8.1 6.7 12.6 10.3 12.4 10 28.5 9.2 42.4 1.2 6.2-3.5 11.2-7.5 15.9-9 9.9-3.1 17.8-8.6 22.3-15 7.7 30.4 25.7 74.3 37.2 95.7 6.1 11.4 18.3 35.5 23.6 64.6 3.3-.1 7 .4 10.9 1.4 13.8-35.7-11.7-74.2-23.3-84.9-4.7-4.6-4.9-6.6-2.6-6.5 12.6 11.2 29.2 33.7 35.2 59 2.8 11.6 3.3 23.7 .4 35.7 16.4 6.8 35.9 17.9 30.7 34.8-2.2-.1-3.2 0-4.2 0 3.2-10.1-3.9-17.6-22.8-26.1-19.6-8.6-36-8.6-38.3 12.5-12.1 4.2-18.3 14.7-21.4 27.3-2.8 11.2-3.6 24.7-4.4 39.9-.5 7.7-3.6 18-6.8 29-32.1 22.9-76.7 32.9-114.3 7.2zm257.4-11.5c-.9 16.8-41.2 19.9-63.2 46.5-13.2 15.7-29.4 24.4-43.6 25.5s-26.5-4.8-33.7-19.3c-4.7-11.1-2.4-23.1 1.1-36.3 3.7-14.2 9.2-28.8 9.9-40.6 .8-15.2 1.7-28.5 4.2-38.7 2.6-10.3 6.6-17.2 13.7-21.1 .3-.2 .7-.3 1-.5 .8 13.2 7.3 26.6 18.8 29.5 12.6 3.3 30.7-7.5 38.4-16.3 9-.3 15.7-.9 22.6 5.1 9.9 8.5 7.1 30.3 17.1 41.6 10.6 11.6 14 19.5 13.7 24.6zM173.3 148.7c2 1.9 4.7 4.5 8 7.1 6.6 5.2 15.8 10.6 27.3 10.6 11.6 0 22.5-5.9 31.8-10.8 4.9-2.6 10.9-7 14.8-10.4s5.9-6.3 3.1-6.6-2.6 2.6-6 5.1c-4.4 3.2-9.7 7.4-13.9 9.8-7.4 4.2-19.5 10.2-29.9 10.2s-18.7-4.8-24.9-9.7c-3.1-2.5-5.7-5-7.7-6.9-1.5-1.4-1.9-4.6-4.3-4.9-1.4-.1-1.8 3.7 1.7 6.5z" />
              </svg>
            )}
          </div>
        )}

        <div className="flex flex-row gap-2 items-left">
          {pypi && (
            <a
              target="_blank"
              href={`https://pypi.org/project/${pypi}/`}
              rel="noreferrer">
              <img
                src={`https://img.shields.io/pypi/v/${pypi}?color=blue`}
                className="h-5"
                alt="PyPI"
              />
            </a>
          )}
          {npm && (
            <a
              target="_blank"
              href={`https://www.npmjs.com/package/${npm}`}
              rel="noreferrer">
              <img
                src={`https://img.shields.io/npm/v/${npm}?color=bf4c4b`}
                className="h-5"
                alt="NPM"
              />
            </a>
          )}
        </div>
      </div>
    );
  };

  const tocHeader = () => {
    return (
      <div className="w-fit">
        <PlatformIcons />
        <div className="flex gap-2 mt-2">
          {github &&
            github.length > 0 &&
            (github.length === 1 ? (
              <a
                href={github[0]}
                rel="noreferrer noopener"
                target="_blank"
                className="inline-flex gap-2 w-fit items-center justify-center rounded-md text-sm font-medium transition-colors duration-100 disabled:pointer-events-none disabled:opacity-50 focus-visible:outline-none hover:bg-fd-accent hover:text-fd-accent-foreground p-1.5 [&amp;_svg]:size-5 text-fd-muted-foreground md:[&amp;_svg]:size-4.5"
                aria-label="Source"
                data-active="false">
                <svg role="img" viewBox="0 0 24 24" fill="currentColor">
                  <path d="M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12"></path>
                </svg>
                Source
                <ExternalLink className="w-4 h-4 ml-auto" />
              </a>
            ) : (
              <Popover>
                <PopoverTrigger className="inline-flex gap-2 w-fit items-center justify-center rounded-md text-sm font-medium transition-colors duration-100 disabled:pointer-events-none disabled:opacity-50 focus-visible:outline-none hover:bg-fd-accent hover:text-fd-accent-foreground p-1.5 [&_svg]:size-5 text-fd-muted-foreground md:[&_svg]:size-4.5">
                  <svg role="img" viewBox="0 0 24 24" fill="currentColor">
                    <path d="M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12"></path>
                  </svg>
                  Source
                  <ChevronDown className="h-4 w-4" />
                </PopoverTrigger>
                <PopoverContent className="w-48 p-1">
                  <div className="flex flex-col gap-1">
                    {github.map((link, index) => (
                      <a
                        key={index}
                        href={link}
                        rel="noreferrer noopener"
                        target="_blank"
                        className="inline-flex gap-2 w-full items-center rounded-md p-2 text-sm hover:bg-fd-accent hover:text-fd-accent-foreground">
                        {link.includes('python')
                          ? 'Python'
                          : link.includes('typescript')
                            ? 'TypeScript'
                            : `Source ${index + 1}`}
                        <ExternalLink className="w-4 h-4 ml-auto" />
                      </a>
                    ))}
                  </div>
                </PopoverContent>
              </Popover>
            ))}
          {slug.includes('libraries') && (
            <a
              className="inline-flex gap-2 w-fit items-center justify-center rounded-md text-sm font-medium transition-colors duration-100 disabled:pointer-events-none disabled:opacity-50 focus-visible:outline-none hover:bg-fd-accent hover:text-fd-accent-foreground p-1.5 [&amp;_svg]:size-5 text-fd-muted-foreground md:[&amp;_svg]:size-4.5"
              href={`/api/${page.data.title.toLowerCase()}`}>
              <CodeXml size={12} />
              Reference
            </a>
          )}
        </div>
        <hr className="my-2 border-t border-fd-border" />
      </div>
    );
  };

  return (
    <DocsPage
      toc={page.data.toc}
      tableOfContent={{ header: tocHeader() }}
      full={page.data.full}>
      <div className="flex flex-row w-full items-start">
        <div className="flex-1">
          <div className="flex flex-row w-full">
            <DocsTitle>{page.data.title}</DocsTitle>

            <div className="ml-auto">
              {apiSection && versionItems.length > 1 && (
                <Popover>
                  <PopoverTrigger
                    className={cn(
                      buttonVariants({
                        color: 'secondary',
                        size: 'sm',
                        className: 'gap-2',
                      })
                    )}>
                    {(() => {
                      // Find the current version label
                      let currentLabel = 'Current';
                      if (apiVersionSlug.length > 0) {
                        const found = versionItems.find(
                          (item) =>
                            item.label !== 'Current' &&
                            apiVersionSlug[0] === item.label
                        );
                        if (found) currentLabel = found.label;
                      }
                      return (
                        <>
                          API Version: {currentLabel}
                          <ChevronDown className="size-3.5 text-fd-muted-foreground" />
                        </>
                      );
                    })()}
                  </PopoverTrigger>
                  <PopoverContent className="flex flex-col overflow-auto">
                    {versionItems.map((item) => {
                      // Build the href for each version
                      const href =
                        item.label === 'Current'
                          ? `/api/${apiSection}`
                          : `/api/${apiSection}/${item.label}`;
                      // Highlight current version
                      const isCurrent =
                        (item.label === 'Current' &&
                          apiVersionSlug.length === 0) ||
                        (item.label !== 'Current' &&
                          apiVersionSlug[0] === item.label);
                      return (
                        <Link
                          key={item.label}
                          href={href}
                          className={cn(
                            'px-3 py-1 rounded hover:bg-fd-muted',
                            isCurrent && 'font-bold bg-fd-muted'
                          )}>
                          API version: {item.label}
                        </Link>
                      );
                    })}
                  </PopoverContent>
                </Popover>
              )}
            </div>
          </div>
          <DocsDescription className="text-md mt-1">
            {page.data.description}
          </DocsDescription>
        </div>
      </div>
      <DocsBody>
        <MDXContent
          components={getMDXComponents({
            // this allows you to link to other pages with relative file paths
            a: createRelativeLink(source, page),
          })}
        />
      </DocsBody>
    </DocsPage>
  );
}

export async function generateStaticParams() {
  return source.generateParams();
}

export async function generateMetadata(props: {
  params: Promise<{ slug?: string[] }>;
}): Promise<Metadata> {
  const params = await props.params;
  const page = source.getPage(params.slug);
  if (!page) notFound();

  let title = `${page.data.title} | C/ua Docs`;
  if (page.url.includes('api')) title = `${page.data.title} | C/ua API Docs`;
  if (page.url.includes('guide'))
    title = ` Guide: ${page.data.title} | C/ua Docs`;

  return {
    title,
    description: page.data.description,
    openGraph: {
      title,
      description: page.data.description,
      type: 'article',
      siteName: 'C/ua Docs',
      url: 'https://trycua.com/docs',
    },
  };
}
