// Time the WebAssembly matcher over perf.rs's corpus, under Node's V8, and
// print tier / pattern / matches / median ms — the same columns to diff
// against a native `./scripts/perf.sh` run. Times include the JSON
// serialization the worker pays and native `search` does not.
//
// Usage: cargo run --release -p cha-core --example perf -- --list-tsv \
//          | node cha-wasm/bench.mjs cha-wasm/dist/cha_wasm.wasm words.txt
import { readFileSync } from "node:fs";

const [wasmPath, wordsPath] = process.argv.slice(2);
const { instance } = await WebAssembly.instantiate(readFileSync(wasmPath), {});
const w = instance.exports;
const enc = new TextEncoder();
const dec = new TextDecoder();

function call(f, s) {
  const b = enc.encode(s);
  const p = w.cha_alloc(b.length);
  new Uint8Array(w.memory.buffer, p, b.length).set(b);
  const r = w[f](p, b.length);
  w.cha_free(p, b.length);
  return r;
}

const t0 = performance.now();
const count = call("cha_load", readFileSync(wordsPath, "utf8"));
console.error(`loaded ${count} words in ${(performance.now() - t0).toFixed(0)} ms`);

const corpus = readFileSync(0, "utf8").trim().split("\n").map((l) => l.split("\t"));
for (const [tier, pattern] of corpus) {
  const once = () => {
    const code = call("cha_search", pattern);
    const out = dec.decode(new Uint8Array(w.memory.buffer, w.cha_out_ptr(), w.cha_out_len()));
    return code === 0 ? JSON.parse(out).total : `ERR ${out}`;
  };
  const matches = once(); // warm-up
  const times = [];
  const deadline = performance.now() + 1000;
  while (times.length < 5 || (performance.now() < deadline && times.length < 200)) {
    const s = performance.now();
    once();
    times.push(performance.now() - s);
  }
  times.sort((a, b) => a - b);
  const med = times[times.length >> 1];
  console.log(`${tier}\t${pattern}\t${matches}\t${med.toFixed(3)}`);
}
