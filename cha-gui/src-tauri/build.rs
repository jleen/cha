use std::path::Path;

fn main() {
    // Embed the word list only when it exists at build time. `include_str!`
    // can't be gated by a runtime `if` (the macro expands unconditionally and
    // would fail to compile if the file were missing), so we gate it on a cfg
    // flag that this script sets based on the file's presence. When it's set,
    // main.rs embeds words.txt; when it isn't, main.rs loads from the user's
    // home dir at runtime.
    println!("cargo::rustc-check-cfg=cfg(words_embedded)");
    // Track the path whether or not it currently exists, so a build is rerun
    // when words.txt later appears (or disappears) and the cfg is re-evaluated.
    println!("cargo::rerun-if-changed=../../words.txt");
    if Path::new("../../words.txt").exists() {
        println!("cargo::rustc-cfg=words_embedded");
    }

    tauri_build::build()
}
