//! Log-file append, console log lines with colors, action records.

use crate::app;
use crate::console::{self, COLOR_CYAN, COLOR_RED, COLOR_WHITE, COLOR_YELLOW};
use crate::winfmt::log_time_stamp;

/// Port of `appendUtf8File` (append/create semantics).
pub fn append_utf8_file(path: &std::path::Path, text: &str) {
    use std::fs::OpenOptions;
    use std::io::Write;
    if path.as_os_str().is_empty() {
        return; // matches CreateFileW("") failing silently pre-init
    }
    let Ok(mut f) = OpenOptions::new().append(true).create(true).open(path) else {
        return;
    };
    let _ = f.write_all(text.as_bytes());
}

fn color_for_level(level: &str) -> u16 {
    match level {
        "ERROR" | "FATAL" => COLOR_RED,
        "WARN" => COLOR_YELLOW,
        "DRYRUN" => COLOR_CYAN,
        _ => COLOR_WHITE,
    }
}

fn no_color() -> bool {
    app::opts(|o| o.no_color)
}

/// Port of `logLine`: console line with timestamp prefix + level color,
/// plus UTF-8 append to the single run log.
pub fn log_line(level: &str, message: &str) {
    let line = format!("[{}] [{}] {message}\n", log_time_stamp(), level);
    let color = color_for_level(level);
    let colored = !no_color();
    console::set_color(color, colored);
    console::out(&line);
    console::set_color(COLOR_WHITE, colored);
    let log_path = app::run(|s| s.log_path.clone());
    append_utf8_file(&log_path, &line);
}

/// Port of `addAction`.
pub fn add_action(kind: &str, status: &str, component: &str, path: &str, detail: &str) {
    app::run_mut(|s| {
        s.actions.push(crate::types::ActionRecord {
            kind: kind.to_string(),
            status: status.to_string(),
            component: component.to_string(),
            path: path.to_string(),
            detail: detail.to_string(),
        });
    });
}

/// Port of `logAction`.
pub fn log_action(kind: &str, status: &str, component: &str, path: &str, detail: &str) {
    add_action(kind, status, component, path, detail);
    let mut msg = format!("{kind} [{component}] {path}");
    if !detail.is_empty() {
        msg.push_str(" :: ");
        msg.push_str(detail);
    }
    log_line(status, &msg);
}
