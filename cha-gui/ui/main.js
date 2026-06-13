// `withGlobalTauri` exposes the API on window.__TAURI__, so the vanilla front
// end can call Rust commands without a bundler or imports.
const { invoke } = window.__TAURI__.core;

const input = document.querySelector("#pattern");
const results = document.querySelector("#results");
const status = document.querySelector("#status");

let timer;

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
  notice.textContent = message;
  results.replaceChildren(notice);
}
checkDict();

input.addEventListener("input", () => {
  clearTimeout(timer);
  timer = setTimeout(run, 100); // debounce keystrokes
});

async function run() {
  const pattern = input.value;
  if (pattern.trim() === "") {
    results.replaceChildren();
    status.textContent = "";
    return;
  }
  try {
    const { matches, total } = await invoke("search", { pattern });
    render(matches, total);
  } catch (e) {
    results.replaceChildren();
    status.textContent = String(e);
    status.classList.add("error");
  }
}

function render(matches, total) {
  status.classList.remove("error");
  const frag = document.createDocumentFragment();
  for (const word of matches) {
    const row = document.createElement("div");
    row.className = "word";
    row.textContent = word;
    frag.appendChild(row);
  }
  results.replaceChildren(frag); // single bulk DOM swap
  results.scrollTop = 0;

  if (total === 0) {
    status.textContent = "no matches";
  } else if (total > matches.length) {
    status.textContent = `showing first ${matches.length} of ${total} matches`;
  } else {
    status.textContent = `${total} match${total === 1 ? "" : "es"}`;
  }
}
