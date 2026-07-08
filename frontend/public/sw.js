// Voice Hub service worker — makes the web app installable (PWA) and gives it
// an offline app shell. Deliberately minimal: it must never interfere with the
// live call path (API, WebSocket) or serve stale bundles.

const CACHE = "voice-hub-v1";
const SHELL = "/"; // start_url — cached on first launch, offline fallback shell

self.addEventListener("install", () => self.skipWaiting());

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (event) => {
  const req = event.request;
  if (req.method !== "GET") return;

  const url = new URL(req.url);
  // Same-origin only; realtime paths always hit the network untouched.
  if (url.origin !== self.location.origin) return;
  if (url.pathname.startsWith("/api") || url.pathname.startsWith("/ws")) return;

  // Navigations: network-first; cache each page under its own URL and fall
  // back offline to that page, then to the shell.
  if (req.mode === "navigate") {
    event.respondWith(
      fetch(req)
        .then((res) => {
          if (res.ok) {
            const copy = res.clone();
            caches.open(CACHE).then((c) => c.put(req, copy));
          }
          return res;
        })
        .catch(() => caches.match(req).then((hit) => hit || caches.match(SHELL))),
    );
    return;
  }

  // Hashed static assets: cache-first (immutable filenames).
  if (url.pathname.startsWith("/assets/") || url.pathname.startsWith("/vendor/")) {
    event.respondWith(
      caches.match(req).then(
        (hit) =>
          hit ||
          fetch(req).then((res) => {
            if (res.ok) {
              const copy = res.clone();
              caches.open(CACHE).then((c) => c.put(req, copy));
            }
            return res;
          }),
      ),
    );
  }
});
