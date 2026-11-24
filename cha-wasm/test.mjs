// Contract tests for a built cha-wasm/dist/: that it is complete, and that
// worker.js answers the three commands exactly as the Tauri app and cha-web do.
// main.js can't tell the transports apart only if these hold.
//
// No browser: worker.js is a classic script, so it runs in a Node `vm` context
// with `self` and `fetch` stubbed, `fetch` reading straight from dist/.
//
// Usage: cha-wasm/build.sh && node cha-wasm/test.mjs
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import vm from "node:vm";

const dist = new URL("dist/", import.meta.url).pathname;
let fails = 0;
function check(name, cond, detail = "") {
  console.log(`${cond ? "  ok  " : "  FAIL"} ${name}${cond ? "" : `  ${detail}`}`);
  if (!cond) fails++;
}

// --- dist/ is complete --------------------------------------------------------
const walk = (d) =>
  readdirSync(d).flatMap((f) => (statSync(join(d, f)).isDirectory() ? walk(join(d, f)) : [join(d, f)]));
const built = walk(dist).map((f) => "./" + relative(dist, f));

const index = readFileSync(join(dist, "index.html"), "utf8");
check("index.html carries the wasm transport marker", index.includes('name="cha-transport" content="wasm"'));
check("index.html links the manifest", index.includes('rel="manifest"'));
check("index.html registers the service worker", index.includes('src="pwa.js"'));

const sw = readFileSync(join(dist, "sw.js"), "utf8");
check("sw.js has no unfilled placeholders", !sw.includes("__CHA_"));
const files = JSON.parse(sw.match(/const FILES = (\[.*\]);/)[1]);
const missing = built.filter((f) => f !== "./sw.js" && !files.includes(f));
check("sw.js precaches every built file", missing.length === 0, missing.join(" "));
const phantom = files.filter((f) => f !== "./" && !built.includes(f));
check("sw.js precaches nothing that isn't built", phantom.length === 0, phantom.join(" "));

const manifest = JSON.parse(readFileSync(join(dist, "manifest.webmanifest"), "utf8"));
const icons = manifest.icons.map((i) => "./" + i.src);
check("manifest icons exist", icons.every((i) => built.includes(i)), icons.join(" "));

// --- worker.js speaks the protocol ---------------------------------------------
const replies = new Map();
const self = { postMessage: (m) => replies.get(m.id)(m) };
const fetchDist = async (url) => {
  const body = readFileSync(join(dist, url));
  const type = url.endsWith(".wasm") ? "application/wasm" : "text/plain";
  return new Response(body, { headers: { "Content-Type": type } });
};
const ctx = vm.createContext({
  self, fetch: fetchDist, WebAssembly, TextEncoder, TextDecoder, performance,
  console: { log() {}, warn: console.warn }, Uint8Array, Promise, JSON, String,
});
vm.runInContext(readFileSync(join(dist, "worker.js"), "utf8"), ctx);
let id = 0;
const send = (command, args) =>
  new Promise((r) => {
    const i = id++;
    replies.set(i, r);
    self.onmessage({ data: { id: i, command, args } });
  });

let r = await send("platform");
check('platform answers "web"', r.ok && r.value === "web", JSON.stringify(r));
r = await send("dict_status");
check("dict_status is null with a word list", r.ok && r.value === null, JSON.stringify(r));
r = await send("search", { pattern: "c.t" });
check("search resolves with a SearchResult", r.ok && r.value.total > 0 && r.value.list_count === 1, JSON.stringify(r).slice(0, 200));
check("rows carry word/unused/extra", r.ok && ["word", "unused", "extra"].every((k) => k in r.value.groups[0].matches[0]));
r = await send("search", { pattern: "c[at" });
check("a bad pattern rejects with a BARE STRING", !r.ok && typeof r.error === "string" && r.error.length > 0, JSON.stringify(r));
r = await send("search", { pattern: "   " });
check("an empty pattern is an empty result, not an error", r.ok && r.value.total === 0, JSON.stringify(r));
r = await send("search", {});
check("a missing pattern rejects with a string", !r.ok && typeof r.error === "string", JSON.stringify(r));
r = await send("nope");
check("an unknown command rejects with a string", !r.ok && typeof r.error === "string", JSON.stringify(r));

if (fails) {
  console.log(`${fails} FAILED`);
  process.exit(1);
}
console.log("ALL CHA-WASM CONTRACT TESTS PASSED");
