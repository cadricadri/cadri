// First-party page-load beacon. Document loads use Navigation Timing. Later
// in-app navigations measure history change to the next frame. Neither sample
// includes API time; that is cadri_http_request_duration_seconds on the engine.
// Labels are route patterns. Memories is "/".

const PAGES = new Set(["/", "/sessions", "/usage", "/team", "/billing"]);

let installed = false;
let documentReported = false;
let acceptingRoutes = false;

export function installPageLoadTelemetry(): void {
  if (typeof window === "undefined" || installed) {
    return;
  }
  installed = true;

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
    const page = pagePattern(window.location.pathname);
    requestAnimationFrame(() => sendPageLoad(page, performance.now() - started));
  });
}

function wrapHistory(name: "pushState" | "replaceState"): void {
  const original = history[name].bind(history);
  history[name] = (data, unused, url) => {
    const started = performance.now();
    const next = pathOf(url);
    const changed = next !== null && next !== window.location.pathname;
    original(data, unused, url);
    if (acceptingRoutes && changed && next) {
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
  if (!Number.isFinite(durationMs) || durationMs < 0) {
    return;
  }
  const body = JSON.stringify({ page, duration_ms: durationMs });
  const blob = new Blob([body], { type: "application/json" });
  if (
    typeof navigator.sendBeacon === "function" &&
    navigator.sendBeacon("/api/telemetry/page", blob)
  ) {
    return;
  }
  void fetch("/api/telemetry/page", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body,
    keepalive: true,
    credentials: "same-origin",
  }).catch(() => undefined);
}
