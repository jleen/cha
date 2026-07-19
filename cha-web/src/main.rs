//! Cha as a web app: the same Rust search engine and the same front end as the
//! desktop and mobile builds, served over HTTP from a single binary.
//!
//! The front end is `cha-gui/ui/` verbatim — embedded at compile time, not
//! copied. It talks to this server through `transport.js`, which POSTs to
//! `/api/<command>` with the same argument shape the Tauri `invoke` bridge uses,
//! so the two backends implement one protocol and the UI can't tell them apart.
//!
//! # Threat model
//!
//! This is currently built for **private / LAN deployment among trusted users**.
//! It bounds the work any one request can cause (see [`web_limits`]) and caps
//! concurrency, because a single pathological pattern would otherwise be able to
//! wedge the process — that much is needed even with no adversary, since a typo
//! can do it. It deliberately does **not** implement rate limiting, request
//! authentication, or TLS. Putting this on the public internet needs a reverse
//! proxy in front and a second look at the limits below.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{DefaultBodyLimit, State};
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Parser;
use tokio::sync::Semaphore;

use cha_core::dictionary::{self, NamedWordList, WordListBuilder};
use cha_core::pattern::CompileLimits;
use cha_core::search::{self, SearchLimits, SearchResult};

/// The word list, embedded when `words.txt` is at the repo root at build time.
/// See build.rs — the decision can't be a runtime `if`.
#[cfg(words_embedded)]
const EMBEDDED_WORDS: Option<&str> = Some(include_str!("../../words.txt"));
#[cfg(not(words_embedded))]
const EMBEDDED_WORDS: Option<&str> = None;

/// The front end, shared byte-for-byte with the Tauri app.
static UI: include_dir::Dir<'_> = include_dir::include_dir!("$CARGO_MANIFEST_DIR/../cha-gui/ui");

/// Rows returned per search. Ten times smaller than the app's 5000 because the
/// binding cost here is the wire, not the DOM: 5000 rows is ~250 KB of JSON per
/// request, and nobody reads past the first screen anyway. `total` is still
/// counted truthfully, so the UI's "showing first N of M" stays honest.
const WEB_MAX_RESULTS: usize = 500;

/// Longest pattern accepted. Real patterns are a handful of characters; this is
/// long enough for anything deliberate and short enough to bound the parser.
const MAX_PATTERN_LEN: usize = 64;

/// Wall-clock ceiling on one scan.
const SEARCH_TIMEOUT: Duration = Duration::from_secs(2);

/// Largest request body accepted. A search request is a few dozen bytes.
const MAX_BODY_BYTES: usize = 8 * 1024;

/// The work ceilings for one request. Much tighter than `CompileLimits::default()`,
/// which is tuned for a local user who can close a slow window; here a slow
/// request occupies a worker that everyone else is waiting on.
fn web_limits() -> SearchLimits {
    SearchLimits {
        compile: CompileLimits {
            max_pattern_len: MAX_PATTERN_LEN,
            max_anagram_combos: 4_096,
            backtrack_limit: 10_000,
            max_fuzzy_steps: 10_000,
        },
        max_results: WEB_MAX_RESULTS,
        deadline: None, // set per request; see `search`
    }
}

/// Every flag also reads an environment variable, so a container can be
/// configured entirely from a compose file's `environment:` block without
/// overriding the command line.
#[derive(Parser)]
#[command(name = "cha-web", about = "Serve Cha as a web app")]
struct Args {
    /// Address to bind. Defaults to loopback: exposing the server should be a
    /// deliberate act, not something that happens because you forgot a flag.
    /// The container image sets `CHA_BIND=0.0.0.0`, which is safe there because
    /// the container's network namespace — not this setting — is what decides
    /// whether anything outside can reach it.
    #[arg(long, env = "CHA_BIND", default_value = "127.0.0.1")]
    bind: IpAddr,

    #[arg(long, env = "CHA_PORT", default_value_t = 8080)]
    port: u16,

    /// Directory of word list files, loaded once at startup. Same rules as the
    /// desktop app's `dictionaries/` folder: every non-hidden regular file, in
    /// sorted order, each labeled by its filename.
    #[arg(long, env = "CHA_DICT_DIR")]
    dict_dir: Option<PathBuf>,

    /// Serve the front end from this directory instead of the embedded copy.
    /// Development convenience — edit `cha-gui/ui/` and reload without a rebuild.
    /// Not for production: it reads whatever is on disk at request time.
    #[arg(long, env = "CHA_UI_DIR")]
    ui_dir: Option<PathBuf>,

    /// Maximum searches running at once. Defaults to the CPU count, since each
    /// one saturates a core.
    #[arg(long, env = "CHA_MAX_CONCURRENT")]
    max_concurrent: Option<usize>,

    /// Probe a already-running server's /healthz on this host's `--port` and
    /// exit 0 (healthy) or 1. Used by the image's HEALTHCHECK so the runtime
    /// stage needs no curl or wget — one static binary, nothing else.
    #[arg(long, hide_short_help = true)]
    health_check: bool,
}

/// One-shot liveness probe: a minimal HTTP/1.0 GET so we need no HTTP client
/// dependency. Always talks to loopback regardless of `--bind`, since it runs
/// inside the same container as the server it's checking.
async fn run_health_check(port: u16) -> ! {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let result = async {
        let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        sock.write_all(b"GET /healthz HTTP/1.0\r\nHost: localhost\r\n\r\n")
            .await?;
        let mut buf = Vec::new();
        sock.read_to_end(&mut buf).await?;
        Ok::<_, std::io::Error>(buf)
    }
    .await;

    match result {
        // Checking the status line rather than the body: a proxy or error page
        // could contain "ok" while the server is failing.
        Ok(buf) if buf.starts_with(b"HTTP/1.0 200") || buf.starts_with(b"HTTP/1.1 200") => {
            std::process::exit(0)
        }
        Ok(_) => {
            eprintln!("health check: unexpected response");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("health check: {e}");
            std::process::exit(1);
        }
    }
}

struct AppState {
    lists: Vec<NamedWordList>,
    /// Admission control. A `try_acquire` that *rejects* rather than a queue that
    /// grows: under more load than the box can serve, a fast 503 lets a client
    /// back off, while an unbounded queue just converts the overload into
    /// unbounded latency and memory.
    permits: Arc<Semaphore>,
    ui_dir: Option<PathBuf>,
}

/// An error rendered the way `transport.js` expects: `{"error": "..."}` with a
/// non-2xx status, which the shim unwraps into a bare string — the same shape
/// Tauri's `invoke` rejects with.
struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

#[derive(serde::Deserialize)]
struct SearchReq {
    // Must stay `pattern` to match `invoke("search", { pattern })` in main.js.
    // Tauri camelCases snake_case argument names on the JS side, so a future
    // two-word argument would need `#[serde(rename)]` here to keep the two
    // transports speaking the same protocol.
    pattern: String,
}

async fn search(
    State(app): State<Arc<AppState>>,
    Json(req): Json<SearchReq>,
) -> Result<Json<SearchResult>, ApiError> {
    let Ok(_permit) = app.permits.clone().try_acquire_owned() else {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Server is busy — try again in a moment".to_string(),
        ));
    };

    let limits = SearchLimits {
        deadline: Some(Instant::now() + SEARCH_TIMEOUT),
        ..web_limits()
    };
    // Hand the scan to a blocking thread. It is CPU-bound for milliseconds to
    // seconds, and on an async worker it would stall every other connection
    // sharing that thread — the same hazard `#[tauri::command(async)]` exists to
    // address in the desktop app, one layer down.
    let state = app.clone();
    tokio::task::spawn_blocking(move || {
        // Move the permit in so it's held for the whole scan, not just until
        // this handler's future yields.
        let _permit = _permit;
        search::search(&state.lists, &req.pattern, &limits)
    })
    .await
    .map_err(|e| {
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Search failed: {e}"),
        )
    })?
    .map(Json)
    .map_err(|e| ApiError(StatusCode::BAD_REQUEST, e.to_string()))
}

/// Always `null` here: unlike the desktop app, a dictionary-less server exits at
/// startup rather than running degraded (see `load_lists`), so by the time this
/// can be called there is always a word list. Kept so the front end's startup
/// path is identical on every transport.
async fn dict_status() -> Json<Option<String>> {
    Json(None)
}

async fn platform() -> Json<&'static str> {
    Json("web")
}

/// Liveness probe for compose/orchestrator healthchecks and for a reverse proxy's
/// upstream check. Deliberately plain text on a fixed path with no dependencies:
/// it must stay cheap and must not queue behind the search semaphore, or a busy
/// server would be reported as unhealthy and restarted exactly when it's under
/// the most load.
async fn healthz() -> &'static str {
    "ok"
}

/// Wait for the signals a container runtime actually sends. `docker stop` sends
/// SIGTERM and waits ~10s before SIGKILL; without a handler the process ignores
/// it and every stop pays that full timeout.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    tracing::info!("shutting down");
}

/// Serve the front end. `/` maps to index.html; everything else is looked up by
/// path. `pattern-syntax.html` and its CSS fall out as ordinary files, so the
/// help sheet works over HTTP with no special handling.
async fn asset(State(app): State<Arc<AppState>>, uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    let body = match &app.ui_dir {
        // --ui-dir: reject any path that escapes the directory. `include_dir`
        // can't be traversed (it only knows the paths compiled into it), but the
        // filesystem can, so this check is why the flag is dev-only in spirit
        // and guarded in fact.
        Some(dir) => {
            let full = dir.join(path);
            match full.canonicalize() {
                Ok(p) if p.starts_with(dir.canonicalize().unwrap_or_else(|_| dir.clone())) => {
                    std::fs::read(&p).ok()
                }
                _ => None,
            }
        }
        None => UI.get_file(path).map(|f| f.contents().to_vec()),
    };

    match body {
        Some(bytes) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref())], bytes).into_response()
        }
        None => (StatusCode::NOT_FOUND, "Not found").into_response(),
    }
}

/// Build the dictionary once at startup: the embedded list, plus every file in
/// `--dict-dir` if given, deduplicated across all of them by `WordListBuilder`.
///
/// Returns `Err` when nothing loaded. The caller exits rather than serving: a
/// desktop app degrades to an empty-dictionary notice because a user is sitting
/// right there with a file manager, but a server's operator is elsewhere and
/// wants to find out at deploy time, not from a confused user.
fn load_lists(dict_dir: Option<&PathBuf>) -> Result<Vec<NamedWordList>, String> {
    let mut builder = WordListBuilder::new();
    if let Some(text) = EMBEDDED_WORDS {
        builder.begin_source("Built-in");
        builder.add_str(text);
    }
    if let Some(dir) = dict_dir {
        if !dir.is_dir() {
            return Err(format!("--dict-dir {} is not a directory", dir.display()));
        }
        dictionary::load_dir(dir, &mut builder);
    }

    let lists = builder.finish_grouped();
    if lists.is_empty() {
        // The container image is built with no embedded list on purpose, so this
        // is the *expected* first-run failure for a container whose dictionary
        // volume isn't mounted or is empty. Say what to do, not just what's wrong.
        return Err(match dict_dir {
            Some(dir) => format!(
                "No word list. {} contains no usable word list files.\n\
                 Add at least one plain-text file (one word per line); hidden \
                 files and subdirectories are ignored.",
                dir.display()
            ),
            None => "No word list, and no --dict-dir was given.\n\
                     This binary has no built-in list (the container image never \
                     embeds one), so a dictionary directory must be supplied: \
                     pass --dict-dir, or set CHA_DICT_DIR and mount a volume there."
                .to_string(),
        });
    }
    Ok(lists)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cha_web=info,tower_http=warn".into()),
        )
        .init();

    let args = Args::parse();

    // Probe mode exits before any dictionary loading — it must stay cheap enough
    // to run every 30s and must not depend on the server's own configuration
    // beyond the port.
    if args.health_check {
        run_health_check(args.port).await;
    }

    let lists = match load_lists(args.dict_dir.as_ref()) {
        Ok(lists) => lists,
        Err(e) => {
            eprintln!("cha-web: {e}");
            std::process::exit(1);
        }
    };
    let total: usize = lists.iter().map(|l| l.words.len()).sum();
    tracing::info!(
        "loaded {} word list(s), {total} words: {}",
        lists.len(),
        lists
            .iter()
            .map(|l| l.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    let max_concurrent = args
        .max_concurrent
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()));

    let state = Arc::new(AppState {
        lists,
        permits: Arc::new(Semaphore::new(max_concurrent)),
        ui_dir: args.ui_dir,
    });

    // The API carries the body limit; asset GETs don't need it.
    let api = Router::new()
        .route("/api/search", post(search))
        .route("/api/dict_status", post(dict_status))
        .route("/api/platform", post(platform))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES));

    // tauri.conf.json sets `"csp": null`, which is fine for a local webview and
    // not for an HTTP origin. The front end has no inline scripts or styles, so
    // 'self' fits with no source changes — keep it that way.
    const CSP: &str = "default-src 'self'; frame-src 'self'; object-src 'none'; base-uri 'none'";

    let app = Router::new()
        .merge(api)
        // Outside the /api router so it carries no body limit and, more
        // importantly, never touches the search semaphore.
        .route("/healthz", get(healthz))
        .fallback(get(asset))
        .layer(
            tower_http::set_header::SetResponseHeaderLayer::if_not_present(
                header::CONTENT_SECURITY_POLICY,
                header::HeaderValue::from_static(CSP),
            ),
        )
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state);

    let addr = SocketAddr::new(args.bind, args.port);
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("cha-web: could not bind {addr}: {e}");
            std::process::exit(1);
        }
    };
    tracing::info!("listening on http://{addr} (max {max_concurrent} concurrent searches)");
    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
    {
        eprintln!("cha-web: server error: {e}");
        std::process::exit(1);
    }
}
