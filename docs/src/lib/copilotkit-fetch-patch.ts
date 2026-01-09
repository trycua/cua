/**
 * CopilotKit Fetch Patch
 *
 * Intercepts POST requests to /api/copilotkit with body {"method":"info"}
 * to cache responses and prevent redundant network requests.
 *
 * This patch MUST be applied before CopilotKit loads to intercept all requests.
 */

// Cache storage
interface CacheEntry {
  response: Response;
  body: string;
  timestamp: number;
}

const CACHE_TTL_MS = 30_000; // 30 seconds
let infoCache: CacheEntry | null = null;
let isPatched = false;

/**
 * Check if a request body matches the CopilotKit info method
 */
function isInfoRequest(body: string | null): boolean {
  if (!body) return false;
  try {
    const parsed = JSON.parse(body);
    return parsed?.method === 'info';
  } catch {
    return false;
  }
}

/**
 * Check if the cached response is still valid
 */
function isCacheValid(): boolean {
  if (!infoCache) return false;
  return Date.now() - infoCache.timestamp < CACHE_TTL_MS;
}

/**
 * Create a cloned Response from the cached data
 */
function createCachedResponse(entry: CacheEntry): Response {
  return new Response(entry.body, {
    status: entry.response.status,
    statusText: entry.response.statusText,
    headers: new Headers(entry.response.headers),
  });
}

/**
 * Apply the fetch patch to intercept CopilotKit info requests.
 *
 * This function is idempotent - safe to call multiple times.
 * Only runs in browser environment.
 *
 * @returns true if patch was applied, false if already patched or not in browser
 */
export function applyCopilotKitFetchPatch(): boolean {
  // Only run in browser
  if (typeof window === 'undefined') {
    return false;
  }

  // Idempotent - don't patch twice
  if (isPatched) {
    return false;
  }

  const originalFetch = window.fetch;

  window.fetch = async function patchedFetch(
    input: RequestInfo | URL,
    init?: RequestInit
  ): Promise<Response> {
    // Determine the URL
    const url =
      typeof input === 'string'
        ? input
        : input instanceof URL
          ? input.toString()
          : input instanceof Request
            ? input.url
            : '';

    // Only intercept POST requests to the copilotkit endpoint
    const isPostRequest = init?.method?.toUpperCase() === 'POST';
    const isCopilotKitEndpoint =
      url.includes('/api/copilotkit') || url.endsWith('/api/copilotkit');

    if (isPostRequest && isCopilotKitEndpoint) {
      // Get the request body
      let bodyString: string | null = null;

      if (typeof init?.body === 'string') {
        bodyString = init.body;
      } else if (init?.body instanceof ArrayBuffer) {
        bodyString = new TextDecoder().decode(init.body);
      } else if (init?.body instanceof Uint8Array) {
        bodyString = new TextDecoder().decode(init.body);
      }

      // Check if this is an info request
      if (isInfoRequest(bodyString)) {
        // Return cached response if valid
        if (isCacheValid() && infoCache) {
          console.debug('[CopilotKit Patch] Returning cached info response');
          return createCachedResponse(infoCache);
        }

        // Make the actual request
        console.debug('[CopilotKit Patch] Making info request (will cache)');
        const response = await originalFetch.call(window, input, init);

        // Clone the response so we can read the body and still return it
        const clonedResponse = response.clone();
        const responseBody = await clonedResponse.text();

        // Cache the response
        infoCache = {
          response: new Response(responseBody, {
            status: response.status,
            statusText: response.statusText,
            headers: new Headers(response.headers),
          }),
          body: responseBody,
          timestamp: Date.now(),
        };

        // Return a new response with the body (original was consumed)
        return new Response(responseBody, {
          status: response.status,
          statusText: response.statusText,
          headers: new Headers(response.headers),
        });
      }
    }

    // Pass through all other requests
    return originalFetch.call(window, input, init);
  };

  isPatched = true;
  console.debug('[CopilotKit Patch] Fetch patch applied');
  return true;
}

/**
 * Clear the info cache manually if needed
 */
export function clearInfoCache(): void {
  infoCache = null;
}

/**
 * Check if the patch has been applied
 */
export function isPatchApplied(): boolean {
  return isPatched;
}
