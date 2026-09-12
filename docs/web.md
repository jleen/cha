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

# Web (`cha-web`, axum)

`cha-web` serves the same engine and the same front end over HTTP, as a single
binary. Run it with `cargo run -p cha-web`; `--dict-dir DIR` adds server-side
word lists, `--ui-dir cha-gui/ui` serves the front end from disk so you can edit
and reload without a rebuild.

- **The front end is `cha-gui/ui/` verbatim, embedded — not copied.** `include_dir!`
  pulls it straight out of the GUI crate at compile time. There is exactly one
  copy of the UI in the repo; if you find yourself duplicating a file, stop.
  Deliberately **not** `rust-embed`, which reads from disk in debug builds unless
  `debug-embed` is set — a debug-built server deployed anywhere would silently
  404 every asset. `build.rs` lists each UI file in `rerun-if-changed`, because
  emitting any such line makes the list exhaustive; **add a line there when you
  add a UI file**, or edits to it won't trigger a rebuild.
- **`spawn_blocking` around the scan is mandatory.** It's CPU-bound for
  milliseconds to seconds, and on an async worker it stalls every other
  connection sharing that thread. This is the same rule as
  `#[tauri::command(async)]` in the GUI, one layer down, with worse symptoms.
  Verified by loading the box with 12 concurrent 2M-word searches and confirming
  static assets still served in 0.2–2.9 ms.
- **A dictionary-less server exits at startup**, unlike the desktop app, which
  degrades to a notice with an "Open Dictionary Folder" button. A user sitting at
  a desktop can fix it; a server operator is elsewhere and wants to know at
  deploy time. `dict_status` therefore always returns `null` on web.
- **`--bind` defaults to `127.0.0.1`.** Exposing the server should be a
  deliberate act, not the result of a forgotten flag.
- **`max_results` is 500 on web, vs 5000 in the app.** The binding cost inverts:
  the app's cap protects the DOM, the server's protects the wire (5000 rows is
  ~250 KB of JSON). `total` is still counted truthfully, so "showing first N of
  M" stays honest.

## The threat model is private/LAN, and that's a decision — not an oversight

The guards below exist because a *typo* can wedge the process; they are not an
adversary story. There is deliberately **no rate limiting, no authentication, and
no TLS**. Putting this on the public internet needs a reverse proxy and a fresh
look at every number here.

| Guard | Value | Where |
|---|---|---|
| Body size | 8 KB | `DefaultBodyLimit` on the `/api` router |
| Pattern length | 64 | `Limits`, via `web_limits()` |
| Anagram combos | 4096 | `Limits` — see the `Limits` section |
| Regex backtracking | 10_000 | `Limits`, per candidate word |
| Fuzzy steps | 10_000 | `Limits`, per candidate word |
| Scan deadline | 2 s | `Limits::deadline`, per 4096-word chunk |
| Concurrency | CPU count | `Semaphore::try_acquire_owned` → 503 |
| CSP | `'self'` | `SetResponseHeaderLayer` |

Two of those choices are easy to get wrong:

- **`try_acquire_owned`, not a queue.** `tower`'s `ConcurrencyLimitLayer` queues,
  and an unbounded queue under overload just converts it into unbounded latency
  and memory. A fast 503 lets a client back off. The permit is moved *into* the
  blocking task so it covers the whole scan.
- **The semaphore is on the search handler only**, so `/api/platform` and asset
  serving stay responsive while the CPU is saturated.

`tauri.conf.json` sets `"csp": null`, which is fine for a local webview and not
for an HTTP origin. The front end has no inline scripts or styles, so `'self'`
fits with no source changes — keep it that way.

Note axum's own extractors (body limit, JSON parse) reject with **plain text**,
not the `{"error": ...}` envelope `ApiError` produces. That's fine — `transport.js`
falls back to the raw body — but don't assume every error response is JSON.

## `--bench` ships in the server on purpose

`cha-web --bench '<pattern>' [--bench-count N]` times a pattern against the
configured dictionary and exits instead of serving, reporting min/mean/max after
one unmeasured warmup pass, plus a verdict against the 2 s request deadline.

It is compiled into the shipped binary deliberately, and that is not an oversight
to tidy up. The server's limits are tighter than the desktop's and only actually
fire on the deployment, against the deployment's real dictionary and CPU share —
numbers from a developer laptop don't transfer. It costs two `Args` fields and one
function, and no dependency.

It answers a different question from
[`cha-core/examples/perf.rs`](../cha-core/examples/perf.rs), which is the
before/after regression suite for the matcher itself (see
[core.md](core.md)): this one answers "is *this machine* too slow, or is *this
pattern* expensive?" on the box that's actually serving.

## Deployment (`deploy/`)

`deploy/` holds the Dockerfile, a compose example, and Caddy + nginx snippets;
`deploy/README.md` is the operator-facing doc. The release workflow publishes
`ghcr.io/<owner>/cha-web` for amd64 and arm64 on every `v*` tag.

- **The image ships no dictionary, and that's enforced explicitly.**
  `CHA_NO_EMBED_WORDS=1` in the Dockerfile makes `build.rs` skip the embed
  regardless of whether `words.txt` is in the build context. Don't "simplify"
  this to just not COPYing words.txt: then whether the image has a dictionary
  depends on an invisible property of the build context, and a missing volume
  mount would be silently masked by a baked-in list. Local `cargo run -p cha-web`
  still embeds, so dev stays zero-config.
- **The Docker build context is the repo root**, not `deploy/`. `cha-web` embeds
  `cha-gui/ui` via `include_dir!`, so the front end must be in context.
- **The image cross-compiles; it does not emulate.** The builder stage is pinned
  to `--platform=$BUILDPLATFORM` and targets `TARGETARCH`, so arm64 is built
  natively on an amd64 runner instead of under QEMU — minutes instead of tens of
  minutes, since rustc is exactly the workload emulation handles worst. **This is
  cheap only because cha-web's tree is pure Rust**: no `-sys` crates, no `cc`, no
  build scripts but cha-web's own. Before adding a dependency with C in it, check
  `cargo tree -p cha-web -e build | grep -iE '\bcc v|cmake|bindgen'`; if that
  finds something, the stage needs a full cross C toolchain plus `CC_<triple>` /
  `AR_<triple>` wiring, and reverting to QEMU may be the better trade. Don't add
  `setup-qemu-action` back to the workflow — the Dockerfile never asks to be
  emulated, so it would silently do nothing.
- **The stub-source layer is a real cache, not decoration.** `cargo build -p
  cha-web` compiles every dependency cha-web *declares*, not just what the stub
  source references, so ~87 crates build in a layer that only invalidates when a
  manifest or the lockfile changes; the real-source layer then rebuilds just
  cha-core and cha-web. Verified end to end.
- **`cha-gui/src-tauri/Cargo.toml` is copied into the build but never built.**
  The workspace manifest lists it as a member, so it must exist for the manifest
  to parse; only `-p cha-web` is built. The stub-source dance in the first stage
  exists so the dependency fetch caches independently of source edits — if you
  add a workspace member, add its manifest and a stub there too or the build
  breaks at the `cargo fetch` layer.
- **`CHA_BIND=0.0.0.0` in the image is correct** and is not a weakening of the
  binary's loopback default. Inside a container the network namespace decides
  reachability; binding loopback there makes the server unreachable even from the
  host.
- **Every flag has an `env` var** (`CHA_PORT`, `CHA_DICT_DIR`, …) so a compose
  file configures it without a custom command line. Add both when adding a flag.
- **`--health-check` probes `/healthz` over loopback and exits 0/1**, so the
  runtime image needs no curl or wget. `/healthz` is routed outside the `/api`
  router deliberately: it must not sit behind the search semaphore, or a busy
  server reports unhealthy exactly when it's under load and gets restarted.
- **SIGTERM is handled** (`with_graceful_shutdown`), so `docker stop` exits
  promptly instead of waiting out its 10s timeout before SIGKILL. Measured at
  ~60 ms.
- **Serve at the root of a host, not a subpath.** The front end fetches `/api/…`
  absolutely, so `example.com/cha/` breaks. Making that work means relative API
  paths and a trailing-slash footgun; not worth it until someone needs it.
- **Don't set a CSP at the proxy.** cha-web sets its own, and multiple CSP
  headers intersect rather than override — the failure mode is a blank page.

---

Back to [AGENTS.md](../AGENTS.md).
