// Register the service worker that makes the static build work offline and
// installable. `serviceWorker` is absent outside a secure context (plain HTTP to
// a LAN address), and the page works fine without it — it just needs the
// network every time — so a missing API is not an error.
if ("serviceWorker" in navigator) {
  navigator.serviceWorker.register("sw.js").catch((e) => console.warn("cha: service worker", e));
}
