import { NextResponse } from 'next/server';
import type { NextRequest } from 'next/server';

// Redirect section roots to their first content page
// These paths don't include basePath because middleware receives paths without it
const redirects: Record<string, string> = {
  '/guide': '/docs/guide/get-started/what-is-cua',
  '/examples': '/docs/examples/automation/form-filling',
  '/reference': '/docs/reference/computer-sdk',
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
