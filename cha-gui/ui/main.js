// Set by transport.js, which picks IPC or HTTP depending on how this page was
// loaded. Same signature either way; nothing below cares which.
const invoke = window.chaInvoke;

const input = document.querySelector("#pattern");
const results = document.querySelector("#results");
const status = document.querySelector("#status");
const helpBtn = document.querySelector("#help");
const helpSheet = document.querySelector("#help-sheet");
const helpFrame = document.querySelector("#help-frame");
const goBtn = document.querySelector("#go");

// How each surface submits a query. Web is the only one where a keystroke costs
// a network round trip and a JSON payload, so it waits for an explicit submit;
// the local apps search live as you type. Switching web to live search is a
// one-line change here (`live: true` with a longer debounce) — nothing else
// needs to move, and #go simply stays hidden.
const SUBMISSION = {
  desktop: { live: true, debounceMs: 100 },
  mobile: { live: true, debounceMs: 100 },
  web: { live: false, debounceMs: 350 },
};

let timer;

// Monotonic id for in-flight searches. Since searches now run concurrently on
// the backend's worker pool, a slow search can resolve after a newer one; we
// stamp each with an id and ignore any result that isn't the latest, so the
// freshest query always wins and the user can keep typing without stale results
// flickering in.
let latestSearch = 0;

// On startup, ask the backend whether a word list is available. If not, show a
// friendly notice (with the exact path the file belongs at) and disable input,
// so an empty result area isn't mistaken for "no matches".
async function checkDict(platform) {
  const message = await invoke("dict_status");
  if (!message) return;
  input.disabled = true;
  input.placeholder = "Word list unavailable";
  const notice = document.createElement("div");
  notice.className = "notice";
  const text = document.createElement("div");
  text.className = "notice-text";
  text.textContent = message;
  notice.replaceChildren(text);
  // `open_dict_dir` is a desktop-only command: mobile has no file manager and no
  // config dir, and on web the dictionary lives on the server where the user has
  // no business browsing. Offering the button anywhere else would reject.
  if (platform === "desktop") {
    const button = document.createElement("button");
    button.className = "notice-button";
    button.textContent = "Open Dictionary Folder";
    button.addEventListener("click", () => invoke("open_dict_dir"));
    notice.appendChild(button);
  }
  results.replaceChildren(notice);
}

// Wire up whichever submission model this surface uses. Called once at startup.
function configureSubmission(platform) {
  const { live, debounceMs } = SUBMISSION[platform];
  if (live) {
    input.addEventListener("input", () => {
      clearTimeout(timer);
      timer = setTimeout(run, debounceMs); // debounce keystrokes
    });
  } else {
    goBtn.hidden = false;
    goBtn.addEventListener("click", run);
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        run();
      }
    });
  }
}

// Bind Ctrl+N to open a new search window. On Windows/Linux the webview swallows
// Ctrl+N before the native menu's accelerator can fire, so we handle it here;
// macOS handles Cmd+N via the native menu (the webview never sees it), so it's
// skipped there to avoid opening two windows. Multiwindow is desktop-only, so
// this never runs on mobile.
//
// The window is created through Tauri's built-in WebviewWindow API rather than a
// custom Rust command: Tauri schedules the creation on the event-loop thread for
// us, avoiding the off-main-thread window-creation hang that a hand-rolled
// command runs into on Windows. The API is destructured inside this function
// rather than at top level so a mobile bundle that omits it can't throw and kill
// the whole script.
function enableNewWindowShortcut() {
  const { WebviewWindow } = window.__TAURI__.webviewWindow;
  window.addEventListener("keydown", (e) => {
    if (e.ctrlKey && !e.shiftKey && !e.altKey && (e.key === "n" || e.key === "N")) {
      e.preventDefault();
      const w = new WebviewWindow(`main-${Date.now()}`, {
        url: "index.html",
        title: "Cha",
        width: 720,
        height: 640,
      });
      w.once("tauri://error", (err) => console.error("new window failed", err));
    }
  });
}

// Open the pattern-syntax cheat sheet in a full-screen sheet. Loaded lazily on
// open, so desktop (which never opens it) never fetches it. Pushing a
// history entry lets Android's hardware Back button close the sheet instead of
// the app; the popstate handler below completes that.
//
// The iframe MUST be navigated with `location.replace`, not by assigning `src`.
// Assigning `src` commits asynchronously and adds an entry to the *joint*
// session history — which lands after the `pushState` below, since that runs
// synchronously on the same tick. `history.back()` in closeHelp then returns to
// the entry where the iframe was still about:blank, so the sheet is blank on
// every subsequent open until a full page reload. `replace()` navigates without
// contributing a history entry at all, which sidesteps the ordering entirely.
function openHelp() {
  helpSheet.hidden = false;
  // Navigate on every open rather than only the first. The sheet is two pages
  // now — the cheat sheet's footer links to license.html — and whichever one
  // the user was last looking at is where the iframe is still parked, so
  // without this the ? button would sometimes open the licenses. Re-navigating
  // costs one local fetch and also resets the scroll position, which is what a
  // freshly opened sheet should do anyway.
  helpFrame.contentWindow.location.replace("pattern-syntax.html");
  input.blur(); // dismiss the on-screen keyboard
  history.pushState({ help: true }, "");
}

function closeHelp() {
  if (helpSheet.hidden) return;
  helpSheet.hidden = true;
  // Undo our own history entry only if it's still on top, so Back and the ✕
  // button converge on the same state.
  if (history.state?.help) history.back();
}

helpBtn.addEventListener("click", openHelp);
document.querySelector("#help-close").addEventListener("click", closeHelp);
window.addEventListener("popstate", () => {
  helpSheet.hidden = true;
});
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") closeHelp();
});

// Cancel the page-level drag iOS offers while the keyboard is up. Preventing
// touchmove cancels the native pan gesture behind it, which is the one lever CSS
// doesn't have: the scroll range is a UIScrollView content inset, not a CSS
// overflow, so no combination of overflow, position or overscroll-behavior
// reaches it (position:fixed on the body was tried and does nothing here — it
// only bought a misplaced caret, since WebKit mispositions the caret and
// selection inside a fixed-position input).
//
// Two things are deliberately let through. Touches inside #results scroll the
// list itself, which is the whole point of shrinking the frame — its
// overscroll-behavior: contain keeps that from chaining out to the page. And
// touches on the focused input belong to iOS's own caret-dragging and selection
// gestures; cancelling those would break dragging the cursor through the field,
// and while the field has focus such a drag scrolls nothing anyway.
function blockPageDrag(e) {
  if (e.target === input) return;
  if (results.contains(e.target) && results.scrollHeight > results.clientHeight) return;
  e.preventDefault();
}

// Measure how much of the frame an on-screen keyboard is covering, publish it as
// --keyboard-inset (see body in styles.css), and while one is up, stop the page
// from being dragged around.
//
// Two separate iOS behaviors meet here, and the fix needs both halves. First,
// iOS never shrinks the layout viewport for the keyboard — window.innerHeight,
// 100vh and 100dvh all stay full-screen — so without the inset the tail of
// #results sits under the keyboard with nothing to bring it up. Second, WKWebView
// and mobile Safari give the page's native scroll view a bottom content inset the
// height of the keyboard, so the page gains that much scroll range even though
// its content fits exactly; that is what let the whole app, input and all, be
// dragged up and down while typing. Shrinking the frame is what makes the second
// half affordable: with nothing stranded under the keyboard, there is no longer
// any reason to want that drag.
//
// The measurement is deliberately platform-agnostic, and self-cancelling wherever
// the keyboard resizes the viewport instead of overlaying it: Android's WebView
// shrinks the layout viewport, so both terms drop together and the inset stays 0
// — no subtraction, no listener, nothing to undo. Multiplying by `scale` converts
// the pinch-zoomed visual viewport back into layout pixels, so zooming alone is
// never mistaken for a keyboard, and the 1px floor ignores rounding noise.
function trackKeyboardInset() {
  const vv = window.visualViewport;
  if (!vv) return; // pre-2019 engines: no visualViewport, no measurement to make
  const root = document.documentElement;
  let inset = 0;
  let open = false;
  const update = () => {
    // What the frame would be with no keyboard — i.e. what CSS's 100dvh resolves
    // to right now — recovered from the body's own box plus whatever we last took
    // off it. Measuring against window.innerHeight instead would be wrong on the
    // web build in mobile Safari, where innerHeight is the *large* viewport (the
    // one that ignores the browser's own toolbars): a permanently visible URL bar
    // would read as a permanently open keyboard. dvh already accounts for browser
    // chrome, so what's left over after subtracting the visual viewport is the
    // keyboard and nothing else.
    const frame = document.body.getBoundingClientRect().height + inset;
    const covered = frame - vv.height * vv.scale;
    inset = covered > 1 ? Math.round(covered) : 0;
    root.style.setProperty("--keyboard-inset", `${inset}px`);

    // Attach the drag block only while a keyboard is actually up. A non-passive
    // touchmove listener opts the page out of WebKit's threaded scrolling, so
    // leaving one attached would tax scrolling #results at every other moment,
    // for a hazard that only exists during those moments.
    const nowOpen = inset > 0;
    if (nowOpen === open) return;
    open = nowOpen;
    if (open) {
      // Put the page back first: WebKit may already have scrolled it while
      // raising the keyboard, and we are about to remove the user's ability to
      // scroll it back by hand.
      window.scrollTo(0, 0);
      document.addEventListener("touchmove", blockPageDrag, { passive: false });
    } else {
      document.removeEventListener("touchmove", blockPageDrag);
    }
  };
  vv.addEventListener("resize", update);
  vv.addEventListener("scroll", update); // fires as the keyboard animates in
  update();
}
trackKeyboardInset();

// Decide the platform-specific surface once at startup. `platform` is compiled
// truth from the backend ("desktop" | "mobile" | "web"), so we never guess from
// the user agent.
async function init() {
  const platform = await invoke("platform");
  document.body.classList.add(platform);

  // The pattern-syntax cheat sheet needs an in-page affordance wherever there's
  // no native menu bar to hang it off: mobile has none, and in a browser tab we
  // don't control one. Only the desktop app routes it through Help > Pattern
  // Syntax, so only desktop hides the button.
  if (platform !== "desktop") helpBtn.hidden = false;

  // Multiwindow is desktop-only, and macOS handles Cmd+N natively (see below).
  if (platform === "desktop" && !navigator.platform.toUpperCase().includes("MAC")) {
    enableNewWindowShortcut();
  }

  configureSubmission(platform);
  await checkDict(platform);
}
init();

async function run() {
  const pattern = input.value;
  if (pattern.trim() === "") {
    latestSearch++; // supersede any in-flight search so its result is dropped
    results.replaceChildren();
    status.textContent = "";
    return;
  }
  const myId = ++latestSearch;
  try {
    const { groups, total, list_count, note } = await invoke("search", { pattern });
    if (myId !== latestSearch) return; // a newer search superseded this one
    render(groups, total, list_count, note);
  } catch (e) {
    if (myId !== latestSearch) return;
    results.replaceChildren();
    status.textContent = String(e);
    status.classList.add("error");
  }
}

function render(groups, total, listCount, note) {
  status.classList.remove("error");

  // A contentless pattern (e.g. a bare `;`) carries a gentle note: show it in the
  // normal status style — like "no matches", never the red error style — and no rows.
  if (note) {
    results.replaceChildren();
    status.textContent = note;
    return;
  }

  // Matches arrive grouped by source word list, in display order. We render each
  // group's rows under an unobtrusive labeled rule — but only when more than one
  // list is loaded; a single-list setup shows no header and looks unchanged.
  const showHeaders = listCount > 1;
  const frag = document.createDocumentFragment();
  let shown = 0;
  for (const g of groups) {
    if (showHeaders) {
      const header = document.createElement("div");
      header.className = "list-header";
      header.textContent = g.list;
      frag.appendChild(header);
    }
    for (const m of g.matches) {
      const row = document.createElement("div");
      row.className = "word";
      row.textContent = m.word;

      const parts = [];
      if (m.unused) parts.push(`−${m.unused}`); // −unused pool letters
      if (m.extra) parts.push(`+${m.extra}`); // +letters not in pool
      if (parts.length) {
        const annot = document.createElement("span");
        annot.className = "word-annot";
        annot.textContent = parts.join(" ");
        row.appendChild(annot);
      }

      frag.appendChild(row);
      shown++;
    }
  }
  results.replaceChildren(frag); // single bulk DOM swap
  results.scrollTop = 0;

  if (total === 0) {
    status.textContent = "no matches";
  } else if (total > shown) {
    status.textContent = `showing first ${shown} of ${total} matches`;
  } else {
    status.textContent = `${total} match${total === 1 ? "" : "es"}`;
  }
}
