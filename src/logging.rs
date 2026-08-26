//! Log-file append, console log lines with colors, action records.

use crate::app;
use crate::console::{self, COLOR_CYAN, COLOR_RED, COLOR_WHITE, COLOR_YELLOW};
use crate::winfmt::log_time_stamp;
use std::sync::atomic::{AtomicBool, Ordering};

/// Named mutex serializing large multi-line appends (candidates list,
/// reports) across all processes of a run so they cannot interleave in the
/// single shared log.
pub const K_LOG_MUTEX_NAME: &str = "Global\\GreenPostInstallDebloatNative_LogMutex";

static APPEND_FAILURE_REPORTED: AtomicBool = AtomicBool::new(false);

fn report_append_failure_once(err: &std::io::Error, path: &std::path::Path) {
    // Escape hatch against recursion: log_line reporting its own append
    // failure must not trigger another failed report.
    if APPEND_FAILURE_REPORTED.swap(true, Ordering::SeqCst) {
        return;
    }
    console::err_out(&format!(
        "ERROR: cannot write to run log {}: {}\n",
        path.display(),
        err
    ));
}

/// Port of `appendUtf8File` (append/create semantics), now fallible so
/// losing the audit trail is observable instead of silent.
pub fn append_utf8_file(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write;
    if path.as_os_str().is_empty() {
        return Ok(()); // matches CreateFileW("") failing silently pre-init
    }
    let mut f = OpenOptions::new().append(true).create(true).open(path)?;
    f.write_all(text.as_bytes())
}

/// Append a LARGE multi-line section while holding a named process-spanning
/// mutex so concurrent launcher/elevated/SYSTEM writers cannot interleave
/// mid-section. Falls back to a direct append if the mutex is unavailable
/// for a bounded grace; append errors are surfaced exactly once.
pub fn append_block_serialized(path: &std::path::Path, text: &str) {
    const WAIT_GRACE_MS: u32 = 30_000;
    let acquired = crate::ffi::try_acquire_named_mutex(K_LOG_MUTEX_NAME, WAIT_GRACE_MS);
    let result = append_utf8_file(path, text);
    drop(acquired); // releases if it was acquired
    if let Err(e) = result {
        report_append_failure_once(&e, path);
    }
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
/// plus UTF-8 append to the single run log. Append failures are surfaced on
/// stderr exactly once per run instead of being swallowed.
pub fn log_line(level: &str, message: &str) {
    let line = format!("[{}] [{}] {message}\n", log_time_stamp(), level);
    let color = color_for_level(level);
    let colored = !no_color();
    console::set_color(color, colored);
    console::out(&line);
    console::set_color(COLOR_WHITE, colored);
    let log_path = app::run(|s| s.log_path.clone());
    if let Err(e) = append_utf8_file(&log_path, &line) {
        report_append_failure_once(&e, &log_path);
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let p = std::env::temp_dir().join(format!(
                "gpd-log-{tag}-{}-{}",
                std::process::id(),
                crate::util::now_unique_suffix()
            ));
            std::fs::create_dir_all(&p).expect("create temp dir");
            TempDir(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn append_reports_io_errors_instead_of_silence() {
        // Appending to a DIRECTORY must return Err, never vanish silently.
        let t = TempDir::new("err");
        assert!(append_utf8_file(&t.0, "x").is_err());
    }

    #[test]
    fn append_creates_and_extends_log() {
        let t = TempDir::new("ok");
        let log = t.0.join("debloat-test.log");
        append_utf8_file(&log, "one\n").expect("first append");
        append_utf8_file(&log, "two\n").expect("second append");
        let text = std::fs::read_to_string(&log).unwrap();
        assert_eq!(text, "one\ntwo\n");
        // Serialized block variant succeeds on the same file.
        append_block_serialized(&log, "block\n");
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.ends_with("block\n"));
    }
}
