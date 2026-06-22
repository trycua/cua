import { NextResponse } from 'next/server';
import type { NextRequest } from 'next/server';

// Redirect section roots to their first content page
// These paths don't include basePath because middleware receives paths without it
const redirects: Record<string, string> = {
  // Redirect root to the new Diátaxis docs entry point
  '/': '/docs/tutorials/drive-your-first-app',
  // Diátaxis section roots
  '/tutorials': '/docs/tutorials/drive-your-first-app',
  '/explanation': '/docs/explanation/what-is-computer-use',
  '/how-to-guides': '/docs/how-to-guides/driver/install',
  '/reference': '/docs/reference/cua-driver/cli-reference',
  // Legacy section redirects (pre-Diátaxis)
  '/cua': '/docs/tutorials/drive-your-first-app',
  '/cua/guide': '/docs/tutorials/drive-your-first-app',
  '/cua/examples': '/docs/how-to-guides/recipes/fill-a-form-from-a-local-file',
  '/cua/reference': '/docs/reference/sandbox-sdk/index',
  '/cua-driver': '/docs/tutorials/drive-your-first-app',
  '/cua-driver/guide': '/docs/tutorials/drive-your-first-app',
  '/cua-driver/reference': '/docs/reference/cua-driver/cli-reference',
  '/cuabench': '/docs/tutorials/drive-your-first-app',
  '/cuabench/guide': '/docs/tutorials/drive-your-first-app',
  '/cuabench/reference': '/docs/reference/cua-driver/cli-reference',
  '/lume': '/docs/tutorials/drive-your-first-app',
  '/lume/guide': '/docs/tutorials/drive-your-first-app',
  '/lume/examples': '/docs/how-to-guides/sandbox/lifecycle',
  '/lume/reference': '/docs/reference/cua-driver/cli-reference',
  // Legacy redirects for old URLs without product prefix
  '/guide': '/docs/tutorials/drive-your-first-app',
  '/examples': '/docs/how-to-guides/recipes/fill-a-form-from-a-local-file',
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
