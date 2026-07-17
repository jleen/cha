// Prevent a console window from opening alongside the GUI on Windows release
// builds. This is a bin-crate attribute, so it stays here rather than in lib.rs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// The whole app lives in the library so mobile's platform shell can load it too;
// this shim is the desktop entry point and does nothing but call in.
fn main() {
    cha_gui_lib::run()
}
