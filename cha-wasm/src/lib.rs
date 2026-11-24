//! The serverless backend: `cha_core::search` compiled to WebAssembly, driven
//! from a Web Worker (`www/worker.js`) that speaks the same three-command
//! protocol as the Tauri app and `cha-web`, so `main.js` can't tell which one it
//! got.
//!
//! A raw `extern "C"` ABI rather than wasm-bindgen, on purpose: the only tool a
//! hacker needs is `rustup target add wasm32-unknown-unknown` — no generated
//! glue, no CLI version to keep in lockstep with a crate. The price is the
//! ~30 lines of string marshalling in worker.js. Strings cross as UTF-8 bytes:
//! JS calls `cha_alloc`, writes, calls in, and frees; results come back through
//! a buffer this module owns, read via `cha_out_ptr`/`cha_out_len` and valid
//! until the next call.
//!
//! State lives in thread-locals because wasm32 has one thread and an exported
//! function has nowhere else to keep it. The worker is the only caller.

use std::cell::RefCell;

use cha_core::dictionary::{NamedWordList, WordListBuilder};
use cha_core::limits::Limits;
use cha_core::search;

thread_local! {
    static LISTS: RefCell<Vec<NamedWordList>> = const { RefCell::new(Vec::new()) };
    static OUT: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Allocate `len` bytes for JS to write a string into.
#[no_mangle]
pub extern "C" fn cha_alloc(len: usize) -> *mut u8 {
    let mut buf = Vec::<u8>::with_capacity(len);
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

/// Free a buffer from `cha_alloc`. `len` must be the length it was allocated with.
///
/// # Safety
/// `ptr` must come from `cha_alloc(len)` and not have been freed.
#[no_mangle]
pub unsafe extern "C" fn cha_free(ptr: *mut u8, len: usize) {
    drop(Vec::from_raw_parts(ptr, 0, len));
}

/// # Safety
/// `ptr..ptr+len` must be valid UTF-8 written by JS into a `cha_alloc` buffer.
unsafe fn arg<'a>(ptr: *const u8, len: usize) -> &'a str {
    // TextEncoder always produces valid UTF-8, so a failure here is a bug in
    // worker.js; render it as an empty string rather than trapping.
    std::str::from_utf8(std::slice::from_raw_parts(ptr, len)).unwrap_or("")
}

/// Replace the dictionary with one list built from `text`. Returns the word count.
///
/// # Safety
/// See [`arg`].
#[no_mangle]
pub unsafe extern "C" fn cha_load(ptr: *const u8, len: usize) -> usize {
    let mut builder = WordListBuilder::new();
    builder.begin_source("Built-in");
    builder.add_str(arg(ptr, len));
    let lists = builder.finish_grouped();
    let count = lists.iter().map(|l| l.words.len()).sum();
    LISTS.with(|l| *l.borrow_mut() = lists);
    count
}

/// Search for the pattern. Returns 0 with the `SearchResult` JSON in the output
/// buffer, or 1 with the error message there — which worker.js rejects with as
/// a bare string, the shape Tauri's `invoke` uses.
///
/// `Limits::interactive()` unchanged, including **no deadline**: `deadline` is an
/// `Instant`, and `Instant::now()` panics on `wasm32-unknown-unknown`. Leaving
/// it `None` means `cha-core` never reads the clock. A runaway pattern pins the
/// worker, never the page.
///
/// # Safety
/// See [`arg`].
#[no_mangle]
pub unsafe extern "C" fn cha_search(ptr: *const u8, len: usize) -> u32 {
    let pattern = arg(ptr, len);
    let result = LISTS.with(|l| search::search(&l.borrow(), pattern, &Limits::interactive()));
    let (code, text) = match result {
        Ok(r) => (
            0,
            serde_json::to_string(&r).unwrap_or_else(|e| e.to_string()),
        ),
        Err(e) => (1, e.to_string()),
    };
    OUT.with(|o| *o.borrow_mut() = text);
    code
}

#[no_mangle]
pub extern "C" fn cha_out_ptr() -> *const u8 {
    OUT.with(|o| o.borrow().as_ptr())
}

#[no_mangle]
pub extern "C" fn cha_out_len() -> usize {
    OUT.with(|o| o.borrow().len())
}
