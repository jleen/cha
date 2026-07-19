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

// Open the pattern-syntax cheat sheet in a full-screen sheet. The iframe's src
// is set lazily on first open, so desktop (which never opens it) never fetches
// it. Pushing a history entry lets Android's hardware Back button close the sheet
// instead of the app; the popstate handler below completes that.
function openHelp() {
  if (!helpFrame.src) helpFrame.src = "pattern-syntax.html";
  helpSheet.hidden = false;
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
