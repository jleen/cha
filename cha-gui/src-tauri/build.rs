use std::path::Path;

fn main() {
    // Embed the word list only when it exists at build time. `include_str!`
    // can't be gated by a runtime `if` (the macro expands unconditionally and
    // would fail to compile if the file were missing), so we gate it on a cfg
    // flag that this script sets based on the file's presence. When it's set,
    // lib.rs embeds words.txt; when it isn't, the desktop app falls back to the
    // user's `dictionaries/` config folder (and a mobile build fails below).
    println!("cargo::rustc-check-cfg=cfg(words_embedded)");
    // Track the path whether or not it currently exists, so a build is rerun
    // when words.txt later appears (or disappears) and the cfg is re-evaluated.
    // Emit this before the possible panic below so the intent survives a future
    // refactor that turns the hard error into a soft one.
    println!("cargo::rerun-if-changed=../../words.txt");

    // Emitting any rerun-if-changed above makes the list exhaustive — cargo stops
    // falling back to "rerun if any file in the package changed" — and
    // tauri_build doesn't register the icons itself. Without this, editing
    // icon.ico rebuilds nothing and the exe keeps its old icon resource.
    println!("cargo::rerun-if-changed=icons/icon.ico");

    let embedded = Path::new("../../words.txt").exists();

    // Desktop tolerates a missing word list: the empty-dictionary notice names
    // the `dictionaries/` folder and offers a button that opens it. Mobile has
    // no config dir, no file manager, and no button — the app would install and
    // launch permanently useless with no way for the user to fix it. Fail the
    // build instead of shipping that. `CARGO_CFG_TARGET_OS` is the target OS in
    // a build script (the same signal tauri-build uses to derive the `mobile`
    // cfg), so this fires even when cross-compiling from a desktop host.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if !embedded && matches!(target_os.as_str(), "android" | "ios") {
        panic!(
            "words.txt not found at the repo root.\n\
             Mobile builds embed the word list and have no `dictionaries/` \
             fallback, so this build would ship an app with no dictionary and \
             no way for the user to add one. Place words.txt at the repo root \
             and rebuild."
        );
    }

    if embedded {
        println!("cargo::rustc-cfg=words_embedded");
    }

    tauri_build::build()
}
