// ─── log.rs ───────────────────────────────────────────────────────────────
// Basic logging support that forwards messages to the browser's JS console.
// Mirrors the familiar `console.log/warn/error` triad so the rest of the
// crate (and panics, via console_error_panic_hook) show up in dev tools.
// ─────────────────────────────────────────────────────────────────────────

/// Log an info-level message to `console.log`.
#[macro_export]
macro_rules! console_log {
    ($($arg:tt)*) => {
        web_sys::console::log_1(&format!($($arg)*).into())
    };
}

/// Log a warning to `console.warn`.
#[macro_export]
macro_rules! console_warn {
    ($($arg:tt)*) => {
        web_sys::console::warn_1(&format!($($arg)*).into())
    };
}

/// Log an error to `console.error`.
#[macro_export]
macro_rules! console_error {
    ($($arg:tt)*) => {
        web_sys::console::error_1(&format!($($arg)*).into())
    };
}
