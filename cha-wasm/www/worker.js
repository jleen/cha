// The serverless backend's host: runs cha_wasm.wasm off the main thread and
// answers the same three commands the Tauri app and cha-web do. transport.js
// posts {id, command, args}; this replies {id, ok, value} or {id, ok:false,
// error} with `error` a bare string — the shape Tauri's invoke rejects with.
//
// A worker, not the page, because a scan is synchronous: on the main thread a
// slow pattern would freeze typing exactly like the desktop app's main thread
// would (see `#[tauri::command(async)]` in lib.rs).
//
// The word list is fetched separately from the module so each is cached on its
// own, and so swapping words.txt on a fork needs no rebuild.

const enc = new TextEncoder();
const dec = new TextDecoder();

let wasm; // the instance's exports
let dictError = null; // dict_status's answer once loading has settled

function withString(s, f) {
  const bytes = enc.encode(s);
  const ptr = wasm.cha_alloc(bytes.length);
  new Uint8Array(wasm.memory.buffer, ptr, bytes.length).set(bytes);
  try {
    return f(ptr, bytes.length);
  } finally {
    wasm.cha_free(ptr, bytes.length);
  }
}

function output() {
  // Re-read memory.buffer every time: any allocation can grow memory, which
  // detaches the old ArrayBuffer.
  const view = new Uint8Array(wasm.memory.buffer, wasm.cha_out_ptr(), wasm.cha_out_len());
  return dec.decode(view);
}

const ready = (async () => {
  const [module, words] = await Promise.all([
    // instantiateStreaming insists on `Content-Type: application/wasm`, which
    // not every static host sends; fall back to buffering rather than failing.
    WebAssembly.instantiateStreaming(fetch("cha_wasm.wasm"), {}).catch(async () =>
      WebAssembly.instantiate(await (await fetch("cha_wasm.wasm")).arrayBuffer(), {}),
    ),
    fetch("words.txt").then((r) => (r.ok ? r.text() : null)),
  ]);
  wasm = module.instance.exports;
  if (words === null) {
    dictError = "No word list found. This build of Cha shipped without words.txt.";
    return;
  }
  const t0 = performance.now();
  const count = withString(words, (p, n) => wasm.cha_load(p, n));
  console.log(`cha: loaded ${count} words in ${(performance.now() - t0).toFixed(0)} ms`);
  if (count === 0) dictError = "No word list found. words.txt is empty.";
})();

const commands = {
  platform: () => "web",
  dict_status: () => dictError,
  search: ({ pattern }) => {
    // Tauri and cha-web both reject a missing argument; TextEncoder would
    // quietly encode `undefined` as an empty search instead.
    if (typeof pattern !== "string") throw "search: missing string argument `pattern`";
    const code = withString(pattern, (p, n) => wasm.cha_search(p, n));
    const text = output();
    if (code !== 0) throw text;
    return JSON.parse(text);
  },
};

self.onmessage = async ({ data: { id, command, args } }) => {
  try {
    await ready;
    const handler = commands[command];
    if (!handler) throw `Unknown command: ${command}`;
    self.postMessage({ id, ok: true, value: handler(args ?? {}) });
  } catch (e) {
    self.postMessage({ id, ok: false, error: typeof e === "string" ? e : String(e) });
  }
};
