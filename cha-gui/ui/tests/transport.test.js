// Contract tests for transport.js's HTTP branch.
//
// The shim's whole job is to be indistinguishable from Tauri's `invoke` to
// everything downstream in main.js. The parts that are easy to get subtly wrong,
// and that these tests pin down:
//
//   - It must reject with a BARE STRING, not an Error. Tauri rejects with the
//     command's `Err` value, and main.js renders failures with `String(e)`; an
//     Error would render as "Error: message" on web and "message" in the app.
//   - It must resolve with parsed JSON, so `const { groups, total } = await ...`
//     destructures the same way on both transports.
//   - It must survive an error body that isn't JSON at all (a reverse proxy's
//     HTML error page), rather than throwing a SyntaxError that masks the real
//     failure.
//
// Run via tests/run.sh, which supplies `window` and `fetch` and then loads
// transport.js into this same global. See AGENTS.md.

var fails = 0;
function check(name, cond) {
  log((cond ? "  ok   " : "  FAIL ") + name);
  if (!cond) fails++;
}

// 1. Success: parsed JSON, and the request is shaped the way cha-web expects.
fetch.next = resp(true, 200, '{"total":3,"groups":[]}');
window
  .chaInvoke("search", { pattern: "c.t" })
  .then(function (r) {
    check("resolves with parsed JSON", r.total === 3);
    check("posts to /api/<command>", lastReq.url === "/api/search");
    check("uses POST", lastReq.opts.method === "POST");
    check("sets a JSON content type", lastReq.opts.headers["Content-Type"] === "application/json");
    check("sends args as the JSON body", lastReq.opts.body === '{"pattern":"c.t"}');

    // 2. Arg-less commands (dict_status, platform) must still send a valid body.
    fetch.next = resp(true, 200, '"web"');
    return window.chaInvoke("platform");
  })
  .then(function (r) {
    check("handles arg-less commands", r === "web" && lastReq.opts.body === "{}");

    // 3. A structured error must surface as the server's message, bare.
    fetch.next = resp(false, 400, '{"error":"Unclosed \'[\' in anagram"}');
    return window.chaInvoke("search", { pattern: "c[at" }).then(
      function () {
        check("rejects on non-2xx", false);
      },
      function (e) {
        check("rejects with a bare string, not an Error", typeof e === "string");
        check("unwraps the server's error message", e === "Unclosed '[' in anagram");
      }
    );
  })
  .then(function () {
    // 4. A non-JSON error body (proxy page) must not throw a SyntaxError.
    fetch.next = resp(false, 502, "<html>Bad Gateway</html>");
    return window.chaInvoke("search", {}).then(
      function () {
        check("rejects on 502", false);
      },
      function (e) {
        check("falls back to the raw body when it isn't JSON", e === "<html>Bad Gateway</html>");
      }
    );
  })
  .then(function () {
    // 5. An empty error body must still produce something the user can act on.
    fetch.next = resp(false, 503, "");
    return window.chaInvoke("search", {}).then(
      function () {
        check("rejects on 503", false);
      },
      function (e) {
        check("synthesizes a message naming the status", /503/.test(e));
      }
    );
  })
  .then(function () {
    // run.sh greps for this marker rather than trusting an exit code, since an
    // unhandled rejection inside a promise chain doesn't reliably set one across
    // both jsc and node.
    log(fails === 0 ? "ALL SHIM CONTRACT TESTS PASSED" : fails + " SHIM CONTRACT TEST(S) FAILED");
  });
