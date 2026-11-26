---
author: Claude (Anthropic)
ai-generated: true
---

> **Provenance:** Written by Claude (Anthropic) while doing the work it
> describes. Reviewed in the normal course of review, but not audited line by
> line — treat specific numbers, paths, and version pins as claims to verify
> rather than guarantees.
>
> See [docs/README.md](README.md).

# Static web build (`cha-wasm`, no server)

A page that downloads everything — the front end, the matcher as WebAssembly,
and the word list — and then searches on the device. It exists so that anyone
can fork Cha, change it, and put the result in front of phone users without an
app store or a server to run: push to a fork with Pages turned on, or rsync one
directory to any static host. The native apps stay the preferred way to run Cha
on a laptop.

```
cha-wasm/build.sh                                   # → cha-wasm/dist/
node cha-wasm/test.mjs                              # contract tests on dist/
python3 -m http.server -d cha-wasm/dist 8765        # try it locally
rsync -a --delete cha-wasm/dist/ host:/srv/cha/     # deploy anywhere
```

The only prerequisite beyond a Rust toolchain is
`rustup target add wasm32-unknown-unknown`. No wasm-bindgen, no Node build step,
no bundler; `build.sh` uses POSIX tools only and runs on macOS too.

## Layout

| Path | What |
|---|---|
| `cha-wasm/src/lib.rs` | `cha_core::search` behind a raw `extern "C"` ABI |
| `cha-wasm/www/worker.js` | Web Worker hosting the module; answers the three commands |
| `cha-wasm/www/sw.js` | service worker template; `build.sh` fills in the build id and file list |
| `cha-wasm/www/head.html` | spliced into the *copied* `index.html`: transport marker and PWA links |
| `cha-wasm/www/pwa.js`, `manifest.webmanifest` | service worker registration, install metadata |
| `cha-wasm/build.sh` | assembles `dist/` |
| `cha-wasm/test.mjs` | `dist/` completeness and worker protocol tests, under Node |
| `cha-wasm/bench.mjs` | times the module over `perf.rs`'s corpus, under Node |
| `.github/workflows/pages.yml` | build, test, publish to GitHub Pages on push to `main` |

`cha-gui/ui/` is copied into `dist/`, never edited for this build: the one shared
`index.html` gets `head.html` spliced into its copy.

Every icon is one a native app already ships, copied rather than resized so
`build.sh` needs no image tools:

| Role | Source |
|---|---|
| Browser tab (`rel="icon"`, 32 and 64 px) | desktop `icons/` |
| Manifest "any" (192, 256, 512 px) | Android's xxxhdpi launcher icon; desktop `icons/` |
| iOS home screen (`apple-touch-icon`, 180 px) | iOS `AppIcon-60x60@3x`. iOS fills transparency with black, and this one is already full-bleed |
| Manifest "maskable" (1024 px) | iOS `AppIcon-512@2x`, full-bleed |

A maskable icon gets cropped to the launcher's shape (often a circle), so its
content must stay within 40% of the width from the centre. Thresholded at
200 px, the glyph's furthest white pixel lands at exactly 0.400, so it survives
the crop but has no margin. If the icon is redrawn, re-check it before keeping
the maskable entry.

## How the front end reaches it

`transport.js` picks one of three transports: Tauri IPC if `window.__TAURI__`
exists, else the worker if the page carries
`<meta name="cha-transport" content="wasm">`, else HTTP to `cha-web`. The marker
is a fact about how the page was built, put there by `build.sh`, so this is not
the device sniffing AGENTS.md forbids.

- **`platform` answers `"web"`**, the same as `cha-web`. It is the same browser
  surface (help button, width cap); only the transport differs.
- **Live search vs. explicit submit is keyed on transport, not platform.**
  `transport.js` sets `window.chaRemote`, true only for HTTP, and `main.js`
  picks its `SUBMISSION` entry from that. Before this build existed, "web" and
  "remote" were the same thing, and `SUBMISSION` was keyed by platform.
- **The worker protocol** is `{id, command, args}` in, `{id, ok, value}` or
  `{id, ok: false, error}` out, with `error` a bare string, the same thing Tauri
  rejects with. `test.mjs` pins it, including rejecting a missing argument as
  Tauri and `cha-web` do, rather than encoding `undefined` as an empty search.
- **A worker, not the page,** because a scan is synchronous; on the main thread a
  slow pattern would freeze typing.

## Why a raw ABI rather than wasm-bindgen

The goal is hackability, and wasm-bindgen adds a CLI that must match the crate's
version exactly, plus generated glue to ship. The raw ABI is five exports
(`cha_alloc`, `cha_free`, `cha_load`, `cha_search`, `cha_out_ptr`/`cha_out_len`)
and about 30 lines of marshalling in `worker.js`. Strings cross as UTF-8; results
come back through a buffer the module owns, valid until the next call.
`worker.js` re-reads `memory.buffer` after every call, because any allocation
can grow memory and detach the old buffer.

## No deadline, and why that is load-bearing

`std::time::Instant::now()` **panics** on `wasm32-unknown-unknown`. `cha-core`
reads the clock only when `Limits::deadline` is `Some`, so `cha-wasm` passes
`Limits::interactive()`, whose deadline is `None`. That needed no change to
`cha-core`, but it is an invariant: a new `Instant::now()` anywhere on the search
path that isn't behind a deadline check would crash the static build at runtime,
and nothing on the host would notice. If the static build ever needs a deadline,
use the `web-time` crate (a drop-in `Instant`), not `std`.

A runaway pattern pins the worker, never the page. The structural and combo
limits still bound it, as everywhere else.

## Service worker and updates

- **Precache everything, serve cache-first.** After the first visit, the app
  works offline and installs to the home screen as a standalone app.
- **The build id is `<version>-<hash of every file in dist/>`.** Any change,
  including only `words.txt`, changes `sw.js`'s bytes, which is what makes
  browsers install the new build. The hash is reproducible: rebuilding unchanged
  sources gives the same id, so a redeploy of the same tree doesn't make every
  client download everything again.
- **Always deploy through `build.sh`.** Copying a new `words.txt` over a deployed
  `dist/` leaves `sw.js` unchanged, and installed copies keep the old list.
- **`cache: "reload"` when precaching**, so a file in the HTTP cache from the
  previous deploy can't be stored under the new build's name.
- **`skipWaiting` + `clients.claim`.** A new build takes over at once instead of
  waiting for every tab to close. That's safe because a page fetches everything
  it needs (main.js, worker, module, word list) at startup, all from one build's
  cache, before the new install can finish. Only the help sheet loads lazily,
  and it stands alone. The visible effect is **one reload of lag**: the first
  load after a deploy runs the old build while installing the new one.
- **Caches are named by registration scope**, so two deployments on one origin
  (`/cha/` and `/cha-hack/` on the same Pages account) never delete each other's.
- **Service workers need a secure context**: HTTPS, or `localhost`. Over plain
  HTTP to a LAN address the page still works, just without offline use or
  install. GitHub Pages and most static hosts provide HTTPS.

## Deploying

**GitHub Pages:** set Settings → Pages → Source to "GitHub Actions". Every push to
`main` then builds, runs `test.mjs`, and publishes. It works the same in a fork.
Everything in `dist/` uses relative URLs, so a project page's `/<repo>/` subpath
works.

**Anywhere else:** `rsync -a --delete cha-wasm/dist/ host:/path/`. What the server
should do:

- Serve over HTTPS, for the service worker.
- Send `.wasm` as `application/wasm`. If it doesn't, the worker falls back to
  buffering the module instead of streaming it, so this only costs a little
  startup time.
- Don't cache `sw.js` for long. Browsers cap this at 24 h anyway, but a short
  `Cache-Control` on it makes deploys show up promptly.

## Measurements (0.7.0 spike, 83,568 words)

All on an i7-14700K under WSL2; the native suite's noise probe said QUIET.
Browser and phone numbers have not been measured.

- **Size:** `.wasm` stripped at opt-level 3 is 1.35 MB raw, 450 KB gzipped;
  `opt-level=s` with LTO gives 1.1 MB / 391 KB. `words.txt` is 221 KB gzipped.
  The regex and Unicode tables dominate the module.
- **Speed:** under Node 24 (V8), `bench.mjs` against native `perf`: 1.14×–1.80×
  native, median 1.34×, two runs within about 3%. The WASM times include JSON
  serialization, which native `search` doesn't pay, so patterns returning
  thousands of rows (`.....`, `*`) show the largest ratios (about 1.77×). The
  protected shapes: `..o..e.` 1.21 → 1.60 ms, `........;gdangboot` 1.76 →
  2.49 ms, `;..oting` 1.96 → 2.98 ms.
- **Correctness:** all 53 corpus entries' match counts equal native.
- **Dictionary load** in the worker: about 15–21 ms.

To repeat the comparison:

```
cargo build --release -p cha-core --example perf
./target/release/examples/perf --list-tsv | node cha-wasm/bench.mjs cha-wasm/dist/cha_wasm.wasm words.txt
./target/release/examples/perf          # native, for the same columns
```

## Relationship to `cha-web`

They overlap: both put the front end in a browser. `cha-web` remains for now. Its
only ongoing cost to this build is that a new command must also be added to
`worker.js`'s `commands` table, making three backends instead of two. The static
build needs no server to run, patch or rate-limit, so `cha-web` is likely to be
deprecated.

---

Back to [AGENTS.md](../AGENTS.md).
