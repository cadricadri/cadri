// First-party page-load beacon. Document loads use Navigation Timing. Later
// in-app navigations measure history change to the next frame. Neither sample
// includes API time; that is cadri_http_request_duration_seconds on the engine.
// Labels are route patterns. Memories is "/".

const PAGES = new Set(["/", "/sessions", "/usage", "/team", "/billing"]);

let installed = false;
let documentReported = false;
let acceptingRoutes = false;
let navigationStarted = 0;
let contentPage = "";

const originalFetch = typeof window !== "undefined" ? window.fetch.bind(window) : null;

export function installPageLoadTelemetry(): void {
  if (typeof window === "undefined" || installed) {
    return;
  }
  installed = true;
  navigationStarted = performance.now();
  if (originalFetch) {
    window.fetch = (input, init) => timedFetch(originalFetch, input, init);
  }

  const reportDocumentLoad = () => {
    if (documentReported) {
      return;
    }
    documentReported = true;
    const timing = performance.getEntriesByType("navigation")[0] as
      | PerformanceNavigationTiming
      | undefined;
    let durationMs = performance.now();
    if (timing && timing.loadEventEnd > 0) {
      durationMs = timing.loadEventEnd - timing.startTime;
    }
    sendPageLoad(pagePattern(window.location.pathname), durationMs);
    acceptingRoutes = true;
  };

  if (document.readyState === "complete") {
    reportDocumentLoad();
  } else {
    window.addEventListener("load", reportDocumentLoad, { once: true });
  }

  wrapHistory("pushState");
  wrapHistory("replaceState");
  window.addEventListener("popstate", () => {
    if (!acceptingRoutes) {
      return;
    }
    const started = performance.now();
    noteNavigation(started);
    const page = pagePattern(window.location.pathname);
    requestAnimationFrame(() => sendPageLoad(page, performance.now() - started));
  });
}

export function reportContentReady(page: string): void {
  // Data often paints before the load event. Page-load samples wait for that
  // event; content samples should not, or the first screen is dropped.
  if (!installed) {
    return;
  }
  const pattern = pagePattern(page);
  if (contentPage === pattern) {
    return;
  }
  contentPage = pattern;
  sendBeacon("/api/telemetry/content", {
    page: pattern,
    duration_ms: performance.now() - navigationStarted,
  });
}

function noteNavigation(at: number): void {
  navigationStarted = at;
  contentPage = "";
}

function wrapHistory(name: "pushState" | "replaceState"): void {
  const original = history[name].bind(history);
  history[name] = (data, unused, url) => {
    const started = performance.now();
    const next = pathOf(url);
    const changed = next !== null && next !== window.location.pathname;
    original(data, unused, url);
    if (acceptingRoutes && changed && next) {
      noteNavigation(started);
      const page = pagePattern(next);
      requestAnimationFrame(() => sendPageLoad(page, performance.now() - started));
    }
  };
}

function pagePattern(pathname: string): string {
  if (pathname === "/sessions" || pathname.startsWith("/sessions/")) {
    return "/sessions";
  }
  if (pathname === "/transcripts" || pathname.startsWith("/transcripts/")) {
    return "/sessions";
  }
  return PAGES.has(pathname) ? pathname : "other";
}

function pathOf(url: string | URL | null | undefined): string | null {
  if (url == null) {
    return null;
  }
  try {
    return new URL(String(url), window.location.href).pathname;
  } catch {
    return null;
  }
}

function sendPageLoad(page: string, durationMs: number): void {
  sendBeacon("/api/telemetry/page", { page, duration_ms: durationMs });
}

function sendBeacon(path: string, payload: { page?: string; path?: string; duration_ms: number }): void {
  if (!Number.isFinite(payload.duration_ms) || payload.duration_ms < 0) {
    return;
  }
  const body = JSON.stringify(payload);
  const blob = new Blob([body], { type: "application/json" });
  if (typeof navigator.sendBeacon === "function" && navigator.sendBeacon(path, blob)) {
    return;
  }
  const post = originalFetch ?? window.fetch.bind(window);
  void post(path, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body,
    keepalive: true,
    credentials: "same-origin",
  }).catch(() => undefined);
}

function timedFetch(
  fetchImpl: typeof window.fetch,
  input: RequestInfo | URL,
  init?: RequestInit,
): Promise<Response> {
  const started = performance.now();
  const path = requestPath(input);
  return fetchImpl(input, init).finally(() => {
    if (path && !path.startsWith("/api/telemetry/")) {
      sendBeacon("/api/telemetry/client", {
        path,
        duration_ms: performance.now() - started,
      });
    }
  });
}

function requestPath(input: RequestInfo | URL): string | null {
  try {
    const raw = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    return new URL(raw, window.location.href).pathname;
  } catch {
    return null;
  }
}
