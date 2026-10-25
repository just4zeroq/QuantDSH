//! QuantDSH desktop carrier (Tauri edition).
//!
//! The binary only forwards to the library entry point so that the whole
//! carrier — including the hidden browser-worker mode — is a single executable,
//! exactly like the Electron build (`workdsh-main.js` is both the app and the
//! worker).

// Silence the unused-crate warning for a binary that only re-exports the lib.
#![allow(clippy::needless_borrow)]
// A GUI application must not open a console window on launch. Without this the
// release binary keeps the default console subsystem, so every start flashed a
// black console (and left a `conhost.exe` behind) — something the Electron
// carrier never did. Debug builds keep the console so diagnostics stay visible.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    workdsh_desktop::run();
}
