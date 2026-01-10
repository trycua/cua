/**
 * CopilotKit Fetch Patch
 * Caches info requests to prevent redundant network requests.
 * Must be applied before CopilotKit loads.
 */

interface CacheEntry {
  response: Response;
  body: string;
  timestamp: number;
}

const CACHE_TTL_MS = 30_000;
let infoCache: CacheEntry | null = null;
let isPatched = false;

function isInfoRequest(body: string | null): boolean {
  if (!body) return false;
  try {
    const parsed = JSON.parse(body);
    return parsed?.method === 'info';
  } catch {
    return false;
  }
}

function isCacheValid(): boolean {
  if (!infoCache) return false;
  return Date.now() - infoCache.timestamp < CACHE_TTL_MS;
}

function createCachedResponse(entry: CacheEntry): Response {
  return new Response(entry.body, {
    status: entry.response.status,
    statusText: entry.response.statusText,
    headers: new Headers(entry.response.headers),
  });
}

export function applyCopilotKitFetchPatch(): boolean {
  if (typeof window === 'undefined' || isPatched) {
    return false;
  }

  const originalFetch = window.fetch;

  window.fetch = async function patchedFetch(
    input: RequestInfo | URL,
    init?: RequestInit
  ): Promise<Response> {
    const url =
      typeof input === 'string'
        ? input
        : input instanceof URL
          ? input.toString()
          : input instanceof Request
            ? input.url
            : '';

    const isPostRequest = init?.method?.toUpperCase() === 'POST';
    const isCopilotKitEndpoint = url.includes('/api/copilotkit');

    if (isPostRequest && isCopilotKitEndpoint) {
      let bodyString: string | null = null;

      if (typeof init?.body === 'string') {
        bodyString = init.body;
      } else if (init?.body instanceof ArrayBuffer || init?.body instanceof Uint8Array) {
        bodyString = new TextDecoder().decode(init.body);
      }

      if (isInfoRequest(bodyString)) {
        if (isCacheValid() && infoCache) {
          return createCachedResponse(infoCache);
        }

        const response = await originalFetch.call(window, input, init);
        const clonedResponse = response.clone();
        const responseBody = await clonedResponse.text();

        infoCache = {
          response: new Response(responseBody, {
            status: response.status,
            statusText: response.statusText,
            headers: new Headers(response.headers),
          }),
          body: responseBody,
          timestamp: Date.now(),
        };

        return new Response(responseBody, {
          status: response.status,
          statusText: response.statusText,
          headers: new Headers(response.headers),
        });
      }
    }

    return originalFetch.call(window, input, init);
  };

  isPatched = true;
  return true;
}
