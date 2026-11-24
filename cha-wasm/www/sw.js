// The static build's service worker: precache every file of one build, serve
// from that cache first, and drop older builds' caches once a new one is in.
//
// build.sh fills in the two placeholders. The cache name carries a hash of every
// file in dist/, so any change at all — code, help text, words.txt — changes
// this file's bytes, which is what makes the browser install a new worker.
// That is also why a deploy must go through build.sh: copying a new words.txt
// over a deployed dist/ by hand leaves the hash, and so every installed copy,
// on the old list.
const BUILD = "__CHA_BUILD__";
const FILES = __CHA_FILES__;

// Scoped by registration so two deployments on one origin (say /cha/ and
// /cha-hack/ on the same GitHub Pages account) never delete each other's caches.
const PREFIX = `cha:${self.registration.scope}:`;
const CACHE = PREFIX + BUILD;

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches
      .open(CACHE)
      // `cache: "reload"` bypasses the HTTP cache. Without it, a file the
      // browser cached from the *previous* deploy could be stored under this
      // build's name, mixing two builds in one cache for good.
      .then((cache) => cache.addAll(FILES.map((f) => new Request(f, { cache: "reload" }))))
      // Take over without waiting for every tab to close. Safe because a page
      // fetches everything it needs at startup — main.js, the worker, the
      // module and the word list all come from one build's cache before this
      // install can finish — so the only file a running page can pick up from
      // the new build is the help sheet, which stands alone.
      .then(() => self.skipWaiting()),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) =>
        Promise.all(
          keys.filter((k) => k.startsWith(PREFIX) && k !== CACHE).map((k) => caches.delete(k)),
        ),
      )
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (event) => {
  if (event.request.method !== "GET") return;
  event.respondWith(
    caches
      .open(CACHE)
      // ignoreSearch: a shared link with a query string is still the same page.
      .then((cache) => cache.match(event.request, { ignoreSearch: true }))
      .then((hit) => hit || fetch(event.request)),
  );
});
