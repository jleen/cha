use std::path::Path;

fn main() {
    // Mirrors cha-gui/src-tauri/build.rs: `include_str!` expands unconditionally
    // and can't be gated by a runtime `if`, so whether words.txt exists has to be
    // decided here and handed to the compiler as a cfg.
    println!("cargo::rustc-check-cfg=cfg(words_embedded)");
    // Track the path whether or not it currently exists, so a build is rerun when
    // words.txt later appears (or disappears) and the cfg is re-evaluated.
    println!("cargo::rerun-if-changed=../words.txt");

    // Unlike a mobile build, a missing words.txt is NOT fatal here: the server
    // can still be run with --dict-dir alone, and an operator who gets it wrong
    // sees the startup error and fixes it. Mobile hard-errors because a shipped
    // app with no dictionary can't be recovered by its user.
    if Path::new("../words.txt").exists() {
        println!("cargo::rustc-cfg=words_embedded");
    }

    // The embedded UI is the front end shared with the Tauri app. Emitting any
    // rerun-if-changed makes the list exhaustive, so every file include_dir! pulls
    // in must be named or edits to them won't trigger a rebuild.
    for f in [
        "index.html",
        "main.js",
        "transport.js",
        "styles.css",
        "pattern-syntax.html",
        "pattern-syntax.css",
    ] {
        println!("cargo::rerun-if-changed=../cha-gui/ui/{f}");
    }
}
