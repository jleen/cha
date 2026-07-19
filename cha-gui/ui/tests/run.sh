#!/bin/sh
# Run the transport.js contract tests (see transport.test.js).
#
# There is no Node dependency in this repo and no bundler, so this deliberately
# uses whatever JS engine happens to be around: macOS ships JavaScriptCore, and
# a Linux dev box usually has node. If neither is present the tests SKIP and this
# exits 0 — the shim is front-end code that no Rust build depends on, so a
# missing JS engine must never fail a build. Nothing in CI invokes this today.
#
# Usage: cha-gui/ui/tests/run.sh
set -eu

dir=$(dirname "$0")
ui=$dir/..

jsc=/System/Library/Frameworks/JavaScriptCore.framework/Versions/A/Helpers/jsc
if [ -x "$jsc" ]; then
  engine=$jsc
elif command -v node >/dev/null 2>&1; then
  engine=node
elif command -v jsc >/dev/null 2>&1; then
  engine=jsc
else
  echo "SKIP: no JS engine found (looked for JavaScriptCore and node)"
  exit 0
fi

# Build one script: a prelude standing in for the browser globals transport.js
# touches, then transport.js itself, then the tests. Concatenating rather than
# importing keeps transport.js free of any module system — it's loaded by a plain
# <script> tag in the real app and must stay that way.
tmp=$(mktemp -t cha-shim-test)
trap 'rm -f "$tmp"' EXIT

cat > "$tmp" <<'PRELUDE'
// --- test prelude: minimal stand-ins for the browser globals -----------------
// `window` with no __TAURI__ is what sends transport.js down its HTTP branch;
// that absence is the entire condition under test.
var window = {};
var lastReq = null;
function fetch(url, opts) {
  lastReq = { url: url, opts: opts };
  return Promise.resolve(fetch.next);
}
function resp(ok, status, body) {
  return {
    ok: ok,
    status: status,
    text: function () { return Promise.resolve(body); },
  };
}
// jsc has print(), node has console.log.
var log = typeof print === "function" ? print : console.log;
PRELUDE

cat "$ui/transport.js" "$dir/transport.test.js" >> "$tmp"

echo "Running transport contract tests with: $engine"
out=$("$engine" "$tmp" 2>&1) || {
  echo "$out"
  echo "FAIL: engine exited non-zero"
  exit 1
}
echo "$out"

# Grep for the marker rather than trusting the exit code: an unhandled rejection
# inside a promise chain doesn't reliably set one across both engines.
case $out in
  *"ALL SHIM CONTRACT TESTS PASSED"*) exit 0 ;;
  *) echo "FAIL: success marker not found"; exit 1 ;;
esac
