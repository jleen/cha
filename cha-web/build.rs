use std::path::Path;

fn main() {
    // Mirrors cha-gui/src-tauri/build.rs: `include_str!` expands unconditionally
    // and can't be gated by a runtime `if`, so whether words.txt exists has to be
    // decided here and handed to the compiler as a cfg.
    println!("cargo::rustc-check-cfg=cfg(words_embedded)");
    // Track the path whether or not it currently exists, so a build is rerun when
    // words.txt later appears (or disappears) and the cfg is re-evaluated.
    println!("cargo::rerun-if-changed=../words.txt");
    println!("cargo::rerun-if-env-changed=CHA_NO_EMBED_WORDS");

    // The published container image ships **no** dictionary: it's mounted at run
    // time so an operator can swap word lists without rebuilding, and so the
    // image doesn't bake in a list whose licensing may not suit every deployer.
    //
    // That has to be explicit rather than implied by words.txt being absent from
    // the build context. Otherwise whether the image contains a dictionary
    // depends on a file the Dockerfile happens not to COPY — invisible, easy to
    // change by accident, and it would silently mask a missing --dict-dir mount.
    // With this set, the build is deterministically dictionary-less.
    let opted_out = std::env::var_os("CHA_NO_EMBED_WORDS").is_some_and(|v| v != "0");

    // Unlike a mobile build, a missing words.txt is NOT fatal here: the server
    // can still be run with --dict-dir alone, and an operator who gets it wrong
    // sees the startup error and fixes it. Mobile hard-errors because a shipped
    // app with no dictionary can't be recovered by its user.
    if !opted_out && Path::new("../words.txt").exists() {
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
