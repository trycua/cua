/**
 * CopilotKit Service Worker
 * Intercepts and deduplicates CopilotKit info requests at the network level.
 * This prevents the infinite request loop bug in CopilotKit v1.50.x with Next.js basePath.
 */

const CACHE_NAME = 'copilotkit-info-cache-v1';
const INFO_CACHE_DURATION_MS = 10000; // 10 seconds

// In-memory cache for quick deduplication
let infoCache = {
  response: null,
  timestamp: 0,
};

// Pending request to handle concurrent requests
let pendingInfoRequest = null;

self.addEventListener('install', (event) => {
  console.log('[CopilotKit SW] Installing service worker');
  self.skipWaiting();
});

self.addEventListener('activate', (event) => {
  console.log('[CopilotKit SW] Activating service worker');
  event.waitUntil(clients.claim());
});

self.addEventListener('fetch', (event) => {
  const request = event.request;
  const url = new URL(request.url);

  // Only intercept POST requests to CopilotKit endpoints
  if (
    request.method !== 'POST' ||
    (!url.pathname.endsWith('/api/copilotkit') && !url.pathname.includes('/api/copilotkit'))
  ) {
    return;
  }

  event.respondWith(handleCopilotKitRequest(request));
});

async function handleCopilotKitRequest(request) {
  try {
    // Clone the request to read the body (can only read once)
    const clonedRequest = request.clone();
    const body = await clonedRequest.text();

    // Check if this is an info request
    let isInfoRequest = false;
    try {
      const parsed = JSON.parse(body);
      isInfoRequest = parsed.method === 'info';
    } catch {
      // Not JSON or invalid, pass through
    }

    // If not an info request, pass through to the network
    if (!isInfoRequest) {
      return fetch(request);
    }

    // Handle info request with caching and deduplication
    return handleInfoRequest(request, body);
  } catch (error) {
    console.error('[CopilotKit SW] Error handling request:', error);
    return fetch(request);
  }
}

async function handleInfoRequest(originalRequest, body) {
  const now = Date.now();

  // Check if we have a valid cached response
  if (infoCache.response && now - infoCache.timestamp < INFO_CACHE_DURATION_MS) {
    console.log('[CopilotKit SW] Returning cached info response');
    return infoCache.response.clone();
  }

  // If there's already a pending request, wait for it
  if (pendingInfoRequest) {
    console.log('[CopilotKit SW] Waiting for pending info request');
    try {
      const response = await pendingInfoRequest;
      return response.clone();
    } catch (error) {
      // If pending request failed, continue to make a new one
      console.log('[CopilotKit SW] Pending request failed, making new request');
    }
  }

  // Make the actual request
  console.log('[CopilotKit SW] Making new info request');
  pendingInfoRequest = fetchAndCacheInfo(originalRequest);

  try {
    const response = await pendingInfoRequest;
    return response.clone();
  } finally {
    pendingInfoRequest = null;
  }
}

async function fetchAndCacheInfo(request) {
  const response = await fetch(request);

  // Only cache successful responses
  if (response.ok) {
    // Clone the response for caching (responses can only be read once)
    const responseToCache = response.clone();

    // Store in memory cache
    infoCache = {
      response: responseToCache,
      timestamp: Date.now(),
    };

    console.log('[CopilotKit SW] Cached info response');
  }

  return response;
}
