# Deploying cha-web

`cha-web` is a single static-ish binary that serves both the API and the front
end. It has no database, no state, no outbound network calls, and writes nothing
to disk. Deployment is correspondingly boring: run the container, mount a
directory of word lists, put a reverse proxy in front of it for TLS.

## Quick start

```sh
mkdir -p dictionaries
cp /path/to/your/words.txt dictionaries/

docker compose -f deploy/docker-compose.yml up -d
curl localhost:8080/healthz          # -> ok
```

Then point a reverse proxy at `127.0.0.1:8080` — see [`Caddyfile`](Caddyfile) or
[`nginx.conf`](nginx.conf).

## Dictionaries

**The image ships no word list.** This is deliberate: word lists have varying
licenses, they're the thing you're most likely to want to change, and baking one
in would mean rebuilding the image to swap it.

Put one or more plain-text files in the mounted directory, one word per line:

```
dictionaries/
  twl.txt          -> a word list labeled "twl"
  sowpods.txt      -> a word list labeled "sowpods"
```

- Each file becomes a separately labeled list; the UI shows a header per list
  when more than one is loaded.
- Files are read **once at startup**. Adding a list needs
  `docker compose restart`.
- Deduplication is global and first-seen-wins, in sorted filename order — a word
  in both `twl.txt` and `sowpods.txt` appears only under `twl`.
- Hidden files (`.DS_Store`) and subdirectories are skipped; an unreadable file
  is logged and skipped rather than fatal.
- **An empty or missing directory is fatal**: the server logs why and exits
  non-zero rather than starting up useless. Check `docker compose logs`.

## Configuration

Every flag has an environment variable, so compose YAML is enough.

| Variable | Default (in image) | Meaning |
|---|---|---|
| `CHA_DICT_DIR` | `/dictionaries` | Where to load word lists from |
| `CHA_BIND` | `0.0.0.0` | Bind address |
| `CHA_PORT` | `8080` | Port |
| `CHA_MAX_CONCURRENT` | CPU count | Simultaneous searches before 503 |
| `CHA_UI_DIR` | *(unset)* | Serve the front end from disk instead of the embedded copy — development only |
| `RUST_LOG` | `cha_web=info,tower_http=warn` | Log filter |

`CHA_BIND=0.0.0.0` is correct **inside a container**: the network namespace, not
this setting, decides what can reach the port, and binding loopback would make
the server unreachable even from the host. Run outside a container and the
default is `127.0.0.1` instead.

If you constrain CPU (the compose file limits to 2.0), set `CHA_MAX_CONCURRENT`
to match. Otherwise the server admits requests based on the host's CPU count and
they contend for less CPU than it thinks it has.

## Security — read this before exposing it publicly

**cha-web has no authentication, no rate limiting, and no TLS.** It was built for
private/LAN use. None of that is an oversight, but none of it is a substitute
for thinking about your situation either.

What it *does* do is bound the work any one request can cause — pattern length,
regex backtracking, anagram expansion, a 2s scan deadline, and a concurrency cap
that returns 503 rather than queueing. Those exist because a *typo* can produce a
pathological pattern, not because of an attacker.

If you put this on the public internet:

- **Terminate TLS at the proxy.** Both example configs do.
- **Add rate limiting.** A search costs real CPU — the usual "a proxy can absorb
  it" intuition doesn't hold. `nginx.conf` includes a `limit_req` zone;
  Caddy needs the `caddy-ratelimit` plugin.
- **Consider putting it behind auth** if it isn't meant to be public at all.
  `Caddyfile` has a commented `basic_auth` block.
- The container runs as a non-root user with `read_only`, `cap_drop: ALL`, and
  `no-new-privileges`. Keep those.

## Reverse proxy notes

cha-web is proxy-neutral: no WebSockets, no sticky sessions, no required
headers, no long-lived connections. Any proxy that can forward HTTP will do.

Two things to know:

- **It must be served at the root of a host or subdomain**, not a subpath. The
  front end fetches `/api/...` absolutely, so `example.com/cha/` would break.
- **Don't add a `Content-Security-Policy` header at the proxy.** cha-web sets its
  own. Multiple CSP headers *intersect* rather than override, so a second one can
  only break things, and the failure looks like "the page is blank for no
  reason".

## Building the image yourself

```sh
docker build -f deploy/Dockerfile -t cha-web .
```

The build context is the **repo root**, not `deploy/` — the build needs
`cha-core`, `cha-web`, and `cha-gui/ui` (the front end is compiled into the
binary). `CHA_NO_EMBED_WORDS=1` is set in the Dockerfile so the image never bakes
in a word list even if `words.txt` is present in your checkout.

The release workflow publishes `ghcr.io/<owner>/cha-web` for `linux/amd64` and
`linux/arm64` on every `v*` tag. Both are **cross-compiled** on an amd64 runner
rather than emulated under QEMU, so the arm64 image costs about as much to build
as the amd64 one. Building arm64 on an amd64 machine locally works the same way:

```sh
docker buildx build --platform linux/arm64 -f deploy/Dockerfile -t cha-web:arm64 .
```
