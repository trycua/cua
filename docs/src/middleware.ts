import { NextResponse } from 'next/server';
import type { NextRequest } from 'next/server';

// Redirect section roots to their first content page
// These paths don't include basePath because middleware receives paths without it
const redirects: Record<string, string> = {
  '/': '/docs/cua/guide/get-started/what-is-cua',
  '/cua': '/docs/cua/guide/get-started/what-is-cua',
  '/cua/guide': '/docs/cua/guide/get-started/what-is-cua',
  '/cua/examples': '/docs/cua/examples/automation/form-filling',
  '/cua/reference': '/docs/cua/reference/computer-sdk',
  '/cuabench': '/docs/cuabench/guide/getting-started/introduction',
  '/cuabench/guide': '/docs/cuabench/guide/getting-started/introduction',
  '/cuabench/reference': '/docs/cuabench/reference/cli-reference',
  '/lume': '/docs/lume/guide/getting-started/introduction',
  '/lume/guide': '/docs/lume/guide/getting-started/introduction',
  '/lume/examples': '/docs/lume/examples/claude-cowork-sandbox',
  '/lume/reference': '/docs/lume/reference/cli-reference',
  // Legacy redirects for old URLs without /cua prefix
  '/guide': '/docs/cua/guide/get-started/what-is-cua',
  '/examples': '/docs/cua/examples/automation/form-filling',
  '/reference': '/docs/cua/reference/computer-sdk',
};

export function middleware(request: NextRequest) {
  const { pathname } = request.nextUrl;

  // Handle section root redirects
  if (redirects[pathname]) {
    return NextResponse.redirect(new URL(redirects[pathname], request.url));
  }

  const requestHeaders = new Headers(request.headers);
  requestHeaders.set('x-pathname', pathname);

  return NextResponse.next({
    request: {
      headers: requestHeaders,
    },
  });
}

export const config = {
  matcher: [
    // Match all paths except static files
    '/((?!_next/static|_next/image|favicon.ico|.*\\.(?:svg|png|jpg|jpeg|gif|webp)$).*)',
  ],
};
