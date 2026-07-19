// How the front end reaches the backend.
//
// This same `ui/` directory is served two ways: bundled into the Tauri app,
// where `withGlobalTauri` injects `window.__TAURI__`, and over HTTP by cha-web,
// where no such object exists. Both expose one `invoke(command, args)` call, so
// picking the transport here is the only place the difference appears — nothing
// downstream in main.js knows which one it got.
//
// Testing for `window.__TAURI__` is capability detection of a *transport*: "is
// the IPC bridge present in this document?" — a fact about how this page was
// loaded, which is directly observable and cannot be wrong. It is NOT user-agent
// platform sniffing, which AGENTS.md forbids. The platform question ("is this
// mobile?") is a separate one, still answered only by the backend's `platform`
// command, which is compiled truth. Conflating the two questions is exactly the
// bug that rule exists to prevent: a desktop browser hitting cha-web has the
// HTTP transport but is not a phone.
window.chaInvoke = window.__TAURI__
  ? window.__TAURI__.core.invoke
  : async function httpInvoke(command, args) {
      const res = await fetch(`/api/${command}`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(args ?? {}),
      });
      const text = await res.text();
      // Tauri's invoke rejects with the command's `Err` value — a bare string,
      // not an Error. Match that exactly, so run()'s `String(e)` renders the
      // server's message rather than "Error: message", and so the latestSearch
      // staleness guard behaves identically on both transports.
      if (!res.ok) {
        let message = text;
        try {
          message = JSON.parse(text).error ?? text;
        } catch {
          // Not JSON (a proxy error page, say) — fall back to the raw body.
        }
        throw message || `Request failed (${res.status})`;
      }
      return text ? JSON.parse(text) : null;
    };
