import { NextRequest, NextResponse } from 'next/server';

export async function GET(
  request: NextRequest,
  { params }: { params: Promise<{ path: string[] }> }
) {
  const { path } = await params;
  const url = new URL(request.url);

  const targetUrl = `${process.env.NEXT_PUBLIC_POSTHOG_HOST}/${path.join('/')}${url.search}`;

  try {
    const response = await fetch(targetUrl, {
      method: 'GET',
      headers: {
        'Content-Type': request.headers.get('Content-Type') || 'application/json',
      },
    });

    // Handle 204 No Content responses
    if (response.status === 204) {
      return new NextResponse(null, { status: 204 });
    }

    const data = await response.arrayBuffer();
    return new NextResponse(data, {
      status: response.status,
      headers: {
        'Content-Type': response.headers.get('Content-Type') || 'application/json',
      },
    });
  } catch (error) {
    console.error('PostHog proxy error:', error);
    return new NextResponse('Error proxying request', { status: 500 });
  }
}

export async function POST(
  request: NextRequest,
  { params }: { params: Promise<{ path: string[] }> }
) {
  const { path } = await params;
  const url = new URL(request.url);

  const targetUrl = `${process.env.NEXT_PUBLIC_POSTHOG_HOST}/${path.join('/')}${url.search}`;

  try {
    const body = await request.arrayBuffer();
    const contentType = request.headers.get('Content-Type') || 'application/x-www-form-urlencoded';

    const response = await fetch(targetUrl, {
      method: 'POST',
      headers: {
        'Content-Type': contentType,
      },
      body,
    });

    // Handle 204 No Content responses
    if (response.status === 204) {
      return new NextResponse(null, { status: 204 });
    }

    const data = await response.arrayBuffer();
    return new NextResponse(data, {
      status: response.status,
      headers: {
        'Content-Type': response.headers.get('Content-Type') || 'application/json',
      },
    });
  } catch (error) {
    console.error('PostHog proxy error:', error);
    return new NextResponse('Error proxying request', { status: 500 });
  }
}
