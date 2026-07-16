// `withGlobalTauri` exposes the API on window.__TAURI__, so the vanilla front
// end can call Rust commands without a bundler or imports.
const { invoke } = window.__TAURI__.core;

const input = document.querySelector("#pattern");
const results = document.querySelector("#results");
const status = document.querySelector("#status");

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
async function checkDict() {
  const message = await invoke("dict_status");
  if (!message) return;
  input.disabled = true;
  input.placeholder = "Word list unavailable";
  const notice = document.createElement("div");
  notice.className = "notice";
  const text = document.createElement("div");
  text.className = "notice-text";
  text.textContent = message;
  const button = document.createElement("button");
  button.className = "notice-button";
  button.textContent = "Open Dictionary Folder";
  button.addEventListener("click", () => invoke("open_dict_dir"));
  notice.replaceChildren(text, button);
  results.replaceChildren(notice);
}
checkDict();

input.addEventListener("input", () => {
  clearTimeout(timer);
  timer = setTimeout(run, 100); // debounce keystrokes
});

// On Windows/Linux the webview swallows Ctrl+N before the native menu's
// accelerator can fire, so bind it here. macOS handles Cmd+N via the native menu
// (the webview never sees it), so we skip it there to avoid opening two windows.
//
// We create the window through Tauri's built-in WebviewWindow API rather than a
// custom Rust command: Tauri schedules the creation on the event-loop thread for
// us, avoiding the off-main-thread window-creation hang that a hand-rolled
// command runs into on Windows.
const { WebviewWindow } = window.__TAURI__.webviewWindow;
const isMac = navigator.platform.toUpperCase().includes("MAC");
if (!isMac) {
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
